// Process CPU-time deltas, not ps %cpu's platform-specific moving average.
export function cpuSeconds(value) {
  const match = /^(?:(\d+)-)?(?:(\d+):)?(\d+):(\d+(?:\.\d+)?)$/.exec(value);
  if (!match) throw new Error("Invalid process CPU time");
  return Number(match[1] || 0) * 86400 + Number(match[2] || 0) * 3600
    + Number(match[3]) * 60 + Number(match[4]);
}

export function parseSamples(text, pids) {
  const result = {};
  for (const line of text.trim().split("\n")) {
    const parts = line.trim().split(/\s+/);
    if (parts.length !== 3) throw new Error("Incomplete process sample");
    const [pid, rss, time] = parts;
    if (!/^\d+$/.test(pid) || !/^\d+$/.test(rss)) throw new Error("Invalid process sample");
    result[pid] = {rss_mib: Number(rss) / 1024, cpu_seconds: cpuSeconds(time)};
  }
  for (const pid of pids) if (!result[pid]) throw new Error(`Measured process ${pid} disappeared`);
  return result;
}

export function summarize(samples, names) {
  if (samples.length < 2) throw new Error("Two process samples are required");
  const first = samples[0], last = samples.at(-1);
  const seconds = (last.monotonic_ms - first.monotonic_ms) / 1000;
  if (!(seconds > 0)) throw new Error("Non-positive sample interval");
  const processes = {};
  for (const [name, pid] of Object.entries(names)) {
    const initial = first.processes[pid], final = last.processes[pid];
    if (!initial || !final) throw new Error(`Missing ${name} sample`);
    const cpu = final.cpu_seconds - initial.cpu_seconds;
    if (cpu < 0) throw new Error("Process CPU time went backwards");
    processes[name] = {
      pid, cpu_percent_one_core: cpu / seconds * 100,
      rss_start_mib: initial.rss_mib, rss_end_mib: final.rss_mib,
      rss_peak_mib: Math.max(...samples.map(s => s.processes[pid].rss_mib)),
      rss_growth_mib: final.rss_mib - initial.rss_mib,
    };
  }
  return {seconds, processes};
}
