export class AccessEnded extends Error {}

export function retryDelay(attempt) { return Math.min(250 * 2 ** Math.min(attempt, 4), 4000); }

export function delay(ms, signal) {
  return new Promise((resolve, reject) => {
    if (signal?.aborted) return reject(signal.reason);
    const abort = () => { clearTimeout(timer); reject(signal.reason); };
    const timer = setTimeout(() => { signal?.removeEventListener("abort", abort); resolve(); }, ms);
    signal?.addEventListener("abort", abort, {once:true});
  });
}

// Parse across arbitrary UTF-8/network boundaries, including split CRLF pairs.
export async function readEvents(body, onEvent, signal) {
  const reader = body.getReader(), decoder = new TextDecoder();
  let buffer = "", lines = [], packetSize = 0;
  try {
    while (!signal.aborted) {
      const {value, done} = await reader.read();
      if (done) return;
      buffer += decoder.decode(value, {stream:true});
      if (buffer.length + packetSize > 8 * 1024 * 1024) throw new Error("Oversized event");
      let end;
      while ((end = buffer.indexOf("\n")) >= 0) {
        const line = buffer.slice(0, end).replace(/\r$/, ""); buffer = buffer.slice(end + 1);
        if (line) { packetSize += line.length + 1; lines.push(line); continue; }
        const packet = lines; lines = []; packetSize = 0;
        const event = packet.find(line => line.startsWith("event:"))?.slice(6).trim() || "message";
        const data = packet.filter(line => line.startsWith("data:")).map(line => line.slice(5).trimStart()).join("\n");
        if (data && !signal.aborted && onEvent({event, data}) === false) return;
      }
    }
  } finally { await reader.cancel().catch(() => {}); reader.releaseLock(); }
}

// A resumed connection requests a fresh canonical frame for the same Session.
// Neither a resume token nor this function can grant authority or send input.
export async function followFeed({url, headers, signal, onEvent, onRetry, fetcher = fetch, sleep = delay}) {
  let attempt = 0;
  while (!signal.aborted) {
    let finished = false;
    try {
      const response = await fetcher(url, {headers, signal, cache:"no-store"});
      if ([401,403,404].includes(response.status)) throw new AccessEnded("Session access ended.");
      if (response.status === 413) throw new AccessEnded("Subscription exceeds the client receive limit; reduce its size and reconnect.");
      if (!response.ok || !response.body) throw new Error("Feed unavailable");
      await readEvents(response.body, message => {
        if (message.event === "ended") throw new AccessEnded(message.data);
        if (message.event === "frame" || message.event === "frame-delta" || message.event === "collection" || message.event === "session") attempt = 0;
        if (message.event === "complete") finished = true;
        onEvent(message);
        return !finished;
      }, signal);
      if (finished || signal.aborted) return;
      throw new Error("Feed disconnected");
    } catch (error) {
      if (signal.aborted) return;
      if (error instanceof AccessEnded) throw error;
      const wait = retryDelay(attempt++);
      onRetry(error, wait);
      try { await sleep(wait, signal); } catch { if (signal.aborted) return; throw error; }
    }
  }
}
