import {AccessEnded, followFeed} from "/stream.mjs";

// Labels are terminal-authored presentation, never HTML or filesystem paths.
const labelText = value => typeof value === "string"
  ? value.replace(/[\u0000-\u001f\u007f-\u009f\u200e\u200f\u202a-\u202e\u2066-\u2069]/g, "").trim() : "";
export function sessionLabel(entry) {
  const directory = labelText(entry.directory);
  const project = directory.replace(/[\\/]+$/, "").split(/[\\/]/).pop() || directory;
  return labelText(entry.title) || project || labelText(entry.executable) || entry.session_id.slice(0, 8);
}
export function sessionDetails(entry) {
  return [labelText(entry.executable), labelText(entry.status)].filter(Boolean).join(" · ");
}

// Keep one current and one in-progress snapshot. No lifetime event log, idle
// timer, or per-session HTTP request. A partial snapshot never proves removal.
export async function followSessions({headers, signal, onSessions, onMode, onStatus, onFeatures = () => {}}) {
  const maximum = 4096;
  let entries = new Map(), pending = null, ready = false;
  const publish = () => onSessions([...entries.values()].sort((a, b) => a.session_id.localeCompare(b.session_id)));
  const invalid = () => { throw new AccessEnded("Invalid session-list update; reconnect explicitly."); };
  const decode = data => { try { return JSON.parse(data); } catch { return invalid(); } };
  await followFeed({
    url:"/sessions/events", headers, signal,
    onRetry:() => {
      pending = null; ready = false;
      onFeatures({workflow_read:false});
      onStatus("Session list disconnected · retrying with the same Share");
    },
    onEvent:({event, data}) => {
      if (event === "configuration") {
        if (data !== "controller" && data !== "observer") return invalid();
        pending = null; ready = false;
        onFeatures({workflow_read:false});
        onMode(data === "controller");
        onStatus("Synchronizing sessions…");
      } else if (event === "features") {
        const features = decode(data);
        if (!features || typeof features.workflow_read !== "boolean") return invalid();
        onFeatures({workflow_read:features.workflow_read});
      } else if (event === "collection") {
        const marker = decode(data);
        if (!marker || typeof marker.generation !== "string" || marker.generation.length !== 36) return invalid();
        if (marker.type === "snapshot_begin") {
          pending = {generation:marker.generation, entries:new Map()};
          ready = false;
          onStatus("Synchronizing sessions…");
        } else if (marker.type === "snapshot_end") {
          if (!pending || pending.generation !== marker.generation) return;
          entries = pending.entries; pending = null; ready = true; publish();
        } else return invalid();
      } else if (event === "session") {
        const entry = decode(data);
        if (!entry || typeof entry.session_id !== "string" || entry.session_id.length !== 36
            || typeof entry.archived !== "boolean" || typeof entry.status !== "string"
            || entry.status.length > 32 || !(entry.executable === null
              || typeof entry.executable === "string" && entry.executable.length <= 512)
            || !(entry.title == null || typeof entry.title === "string" && entry.title.length <= 1024)
            || !(entry.directory == null || typeof entry.directory === "string" && entry.directory.length <= 4096)) return invalid();
        if (!pending && !ready) return;
        const target = pending ? pending.entries : entries;
        if (entry.archived) target.delete(entry.session_id);
        else {
          if (!target.has(entry.session_id) && target.size >= maximum) {
            throw new AccessEnded("Browser session list exceeds 4,096 entries; narrow the Share and reconnect.");
          }
          const previous = target.get(entry.session_id);
          if (previous?.status === entry.status && previous?.executable === entry.executable
              && previous?.title === entry.title && previous?.directory === entry.directory) return;
          target.set(entry.session_id, entry);
        }
        if (!pending) publish();
      }
    },
  });
}
