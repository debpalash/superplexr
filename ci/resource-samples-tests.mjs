import test from "node:test";
import assert from "node:assert/strict";
import {cpuSeconds, parseSamples, summarize} from "./resource-samples.mjs";

test("CPU counters support Darwin fractions and Linux day/hour formats", () => {
  assert.equal(cpuSeconds("0:01.25"), 1.25);
  assert.equal(cpuSeconds("01:02:03"), 3723);
  assert.equal(cpuSeconds("2-01:02:03"), 176523);
  assert.throws(() => cpuSeconds("unknown"));
});
test("missing processes cannot silently become zero CPU or memory", () => {
  assert.deepEqual(parseSamples(" 12 2048 0:01.00\n", [12]), {12:{rss_mib:2,cpu_seconds:1}});
  assert.throws(() => parseSamples("12 2048 0:01.00", [12,13]));
  assert.throws(() => parseSamples("12 unknown 0:01.00", [12]));
});
test("summaries use elapsed process CPU, one-core units and sampled RSS peak", () => {
  const samples = [
    {monotonic_ms:100,processes:{12:{rss_mib:10,cpu_seconds:20}}},
    {monotonic_ms:150100,processes:{12:{rss_mib:14,cpu_seconds:20.5}}},
    {monotonic_ms:300100,processes:{12:{rss_mib:11,cpu_seconds:21.5}}},
  ];
  const result = summarize(samples, {runtime:12});
  assert.equal(result.seconds,300);
  assert.equal(result.processes.runtime.cpu_percent_one_core,0.5);
  assert.equal(result.processes.runtime.rss_peak_mib,14);
  assert.equal(result.processes.runtime.rss_growth_mib,1);
  assert.throws(() => summarize([samples[0]], {runtime:12}));
  assert.throws(() => summarize([samples[2],samples[0]], {runtime:12}));
});
