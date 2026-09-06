const uuid = value => typeof value === "string" && /^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/i.test(value);
const object = value => value !== null && typeof value === "object" && !Array.isArray(value);
const execution = value => object(value) && ["pending","running","paused","finished"].includes(value.phase)
  && (value.outcome === null || ["Succeeded","Failed","Cancelled"].includes(value.outcome))
  && ["none","awaiting_review","accepted","rejected"].includes(value.subject_disposition);
async function readJson(response, signal) {
  if (!response.body) throw new Error("Workflow response was empty; retry explicitly.");
  const reader = response.body.getReader(), decoder = new TextDecoder();
  const cancel = () => { reader.cancel().catch(() => {}); };
  signal.addEventListener("abort", cancel, {once:true});
  let text = "", bytes = 0;
  try {
    while (true) {
      const {done, value} = await reader.read();
      if (done) break;
      bytes += value.byteLength;
      if (bytes > 65536) throw new Error("Workflow response exceeded its limit.");
      text += decoder.decode(value, {stream:true});
    }
    if (signal.aborted) throw new Error("Workflow read cancelled.");
    return JSON.parse(text + decoder.decode());
  } finally { signal.removeEventListener("abort", cancel); await reader.cancel().catch(() => {}); reader.releaseLock(); }
}

// One explicit read, no background refresh and no executable next-action UI.
export class WorkflowInspector {
  constructor(root, {headers, fetcher = globalThis.fetch}) {
    this.root = root; this.headers = headers; this.fetcher = fetcher; this.generation = 0;
    this.form = root.querySelector("form");
    this.mission = root.querySelector("[name=mission]");
    this.verifier = root.querySelector("[name=verifier]");
    this.message = root.querySelector("[role=status]");
    this.result = root.querySelector(".workflow-result");
    this.discoverButton = root.querySelector(".workflow-discover");
    this.inspectButton = root.querySelector("button[type=submit]");
    this.page = root.querySelector(".workflow-page");
    this.pageInfo = root.querySelector(".workflow-page-info");
    this.entries = root.querySelector(".workflow-verifiers");
    this.nextButton = root.querySelector(".workflow-next");
    this.form.addEventListener("submit", event => { event.preventDefault(); this.inspect(); });
    this.form.addEventListener("input", event => this.clear("IDs changed · list or inspect explicitly", event.target !== this.verifier));
    this.discoverButton.addEventListener("click", () => this.discover());
    this.nextButton.addEventListener("click", () => { if (this.nextAfter) this.discover(this.nextAfter); });
    root.addEventListener("toggle", () => { if (!root.open) this.clear(); });
    this.setEnabled(false);
  }
  clear(message = "Read-only · enter a Mission ID to list verifiers, or both IDs to inspect", clearPage = true) {
    this.generation++; this.abort?.abort(); this.abort = null;
    this.result.replaceChildren(); this.message.textContent = message;
    if (clearPage) {
      this.entries.replaceChildren(); this.pageInfo.textContent = "";
      this.page.hidden = true; this.nextAfter = null; this.pageMission = null;
    }
    this.updateButtons();
  }
  updateButtons() {
    const disabled = !this.enabled || !!this.abort;
    this.inspectButton.disabled = disabled; this.discoverButton.disabled = disabled;
    this.nextButton.disabled = disabled || !this.nextAfter;
    for (const button of this.entries.querySelectorAll("button")) button.disabled = disabled;
  }
  setEnabled(enabled) {
    this.enabled = enabled; this.root.hidden = !enabled;
    this.clear();
    if (!enabled) { this.mission.value = ""; this.verifier.value = ""; this.root.open = false; }
  }
  async inspect() {
    if (!this.enabled) return;
    this.clear(undefined, this.pageMission !== this.mission.value.trim().toLowerCase());
    const mission = this.mission.value.trim().toLowerCase(), verifier = this.verifier.value.trim().toLowerCase();
    if (!uuid(mission) || !uuid(verifier)) { this.message.textContent = "Enter valid Mission and verifier Run IDs."; return; }
    await this.read(`/missions/${mission}/verifiers/${verifier}/status`, "Reading workflow status…", status => {
      if (!execution(status) || status.mission_id !== mission || status.verifier_run_id !== verifier
          || !Number.isSafeInteger(status.receipt_count) || status.receipt_count < 0
          || !Number.isSafeInteger(status.passing_receipt_count) || status.passing_receipt_count < 0
          || status.passing_receipt_count > status.receipt_count
          || !Array.isArray(status.receipts) || status.receipts.length > 16
          || status.receipts.length > status.receipt_count
          || typeof status.receipts_truncated !== "boolean"
          || !uuid(status.subject_run_id) || !/^[0-9a-f]{64}$/i.test(status.candidate_sha256)
          || typeof status.candidate_revision !== "string" || status.candidate_revision.length > 256
          || status.evidence_rechecked !== false || status.observation_only !== true) {
        throw new Error("Invalid workflow status; refresh explicitly.");
      }
      const facts = document.createElement("dl");
      const fact = (name, value) => {
        const label = document.createElement("dt"), text = document.createElement("dd");
        label.textContent = name; text.textContent = value; facts.append(label, text);
      };
      fact("Execution", `${status.phase} · ${status.outcome || "no finished outcome"}`);
      fact("Recorded receipts", `${status.receipt_count} total · ${status.passing_receipt_count} match the domain passing predicate`);
      fact("Subject disposition", status.subject_disposition);
      fact("Subject Run", status.subject_run_id);
      fact("Candidate revision", status.candidate_revision);
      fact("Candidate SHA-256", status.candidate_sha256);
      const receipts = document.createElement("ul");
      for (const receipt of status.receipts) {
        if (!object(receipt) || !uuid(receipt.artifact_id) || !["passed","failed","inconclusive"].includes(receipt.verdict)) {
          throw new Error("Invalid receipt summary; refresh explicitly.");
        }
        const item = document.createElement("li");
        item.textContent = `${receipt.artifact_id} · ${receipt.verdict}`;
        receipts.append(item);
      }
      const note = document.createElement("p");
      note.textContent = "Recorded facts only; evidence files were not rechecked. Successful execution does not mean checks passed or work was accepted."
        + (status.receipts_truncated ? " Only the first 16 receipt summaries are shown." : "");
      this.result.append(facts, receipts, note);
    });
  }
  async discover(after = null) {
    if (!this.enabled) return;
    const mission = this.mission.value.trim().toLowerCase();
    if (after !== null && (!uuid(after) || this.pageMission !== mission || after !== this.nextAfter)) return;
    this.clear();
    if (!uuid(mission)) { this.message.textContent = "Enter a valid Mission ID to list verifiers."; return; }
    const limit = 32;
    await this.read(`/missions/${mission}/verifiers?limit=${limit}${after ? `&after=${after}` : ""}`, "Reading verifier list…", page => {
      if (!object(page) || page.mission_id !== mission || !Number.isSafeInteger(page.mission_version) || page.mission_version < 0
          || page.after !== after || page.limit !== limit || !Array.isArray(page.entries) || page.entries.length > limit
          || !(page.next_after === null || uuid(page.next_after))) throw new Error("Invalid verifier page; list again explicitly.");
      let previous = after;
      for (const entry of page.entries) {
        if (!execution(entry) || !uuid(entry.verifier_run_id) || !uuid(entry.subject_run_id)
            || !(entry.primary_session_id === null || uuid(entry.primary_session_id))
            || (previous !== null && entry.verifier_run_id <= previous)) throw new Error("Invalid verifier entry; list again explicitly.");
        previous = entry.verifier_run_id;
      }
      if (page.next_after !== null && (page.entries.length !== limit || page.next_after !== previous)) {
        throw new Error("Invalid verifier cursor; list again explicitly.");
      }
      const items = page.entries.map(entry => {
        const item = document.createElement("li"), select = document.createElement("button"), facts = document.createElement("span");
        select.type = "button"; select.textContent = `Inspect ${entry.verifier_run_id}`;
        select.addEventListener("click", () => {
          if (!this.enabled || this.abort || this.pageMission !== mission || this.mission.value.trim().toLowerCase() !== mission) return;
          this.verifier.value = entry.verifier_run_id; this.inspect();
        });
        facts.textContent = `${entry.phase} · ${entry.outcome || "no finished outcome"} · subject ${entry.subject_run_id} · ${entry.subject_disposition}`
          + (entry.primary_session_id ? ` · session ${entry.primary_session_id}` : "");
        item.append(select, facts); return item;
      });
      this.entries.replaceChildren(...items); this.nextAfter = page.next_after; this.pageMission = mission;
      this.pageInfo.textContent = `${page.entries.length} verifiers · Mission version ${page.mission_version}`
        + (after ? " · continuation page" : " · first page") + (page.next_after ? " · more available" : " · end of list");
      this.page.hidden = false;
    });
  }
  async read(url, message, render) {
    const generation = this.generation, abort = new AbortController(); this.abort = abort;
    this.updateButtons(); this.message.textContent = message;
    const timer = setTimeout(() => abort.abort(), 10000);
    let response;
    try {
      const fetcher = this.fetcher;
      response = await fetcher(url, {headers:this.headers(), signal:abort.signal, cache:"no-store", method:"GET"});
      if (generation !== this.generation) return;
      if (abort.signal.aborted) throw new Error("Workflow read timed out.");
      if ([401,403].includes(response.status)) throw new Error("Workflow unavailable; check the gateway option, access key and Share Mission scope.");
      if (!response.ok && !response.headers.get("content-type")?.includes("application/json")) {
        throw new Error(`Workflow request failed (${response.status}); refresh explicitly.`);
      }
      const data = await readJson(response, abort.signal);
      if (generation !== this.generation) return;
      if (abort.signal.aborted) throw new Error("Workflow read timed out.");
      if (!response.ok) throw new Error(object(data) && typeof data.error === "string" ? data.error.slice(0, 512) : "Workflow read is unavailable.");
      render(data);
      this.message.textContent = `Snapshot read at ${new Date().toLocaleTimeString()} · no automatic refresh`;
    } catch (error) {
      if (generation === this.generation) this.message.textContent = abort.signal.aborted
        ? "Workflow read timed out; retry explicitly." : String(error.message || "Workflow read failed; retry explicitly.").slice(0,512);
    } finally {
      clearTimeout(timer);
      if (response?.body && !response.body.locked) await response.body.cancel().catch(() => {});
      if (generation === this.generation) { this.abort = null; this.updateButtons(); }
    }
  }
}
