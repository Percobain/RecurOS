import raw from "@/data/benchmarks.json";

/** Everything the site claims, measured by scripts/bench.py against this
 *  repository's own store. Nothing here is hand-written. */
export type Benchmarks = {
  pack_tokens: number;
  pack_claims: number;
  prose: { file: string; tokens: number }[];
  prose_tokens: number;
  code_tokens: number;
  code_files: number;
  question: string;
  answer_tokens: number;
  answer_source: string;
  answer_source_tokens: number;
  ms_startup: number;
  ms_pack: number;
  ms_search: number;
  reindex: string;
  claims_indexed: number;
  ms_reindex: number;
  log_bytes: number;
  measured_at: string;
  platform: string;
};

export const bench = raw as Benchmarks;

export const fmt = (n: number) => n.toLocaleString("en-US");

/** How many times bigger a is than b, to one decimal. */
export const ratio = (a: number, b: number) => Math.round((a / b) * 10) / 10;

export const kb = (bytes: number) => `${Math.round(bytes / 1024)} KB`;
