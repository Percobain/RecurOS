export const metadata = { title: "Command reference" };

type Cmd = { cmd: string; what: string };

const GROUPS: { title: string; note?: string; rows: Cmd[] }[] = [
  {
    title: "Recording",
    note: "Instant and offline. No network, no model call.",
    rows: [
      { cmd: 'ctx save "<what>" -k decision -w "<why>"', what: "Record a decision." },
      {
        cmd: "-k constraint | rejected | fact | question | claim",
        what: "The other five kinds.",
      },
      { cmd: "-r path/to/file.rs,https://…", what: "What it is about. Also how search finds it." },
      { cmd: "-t billing,auth", what: "Topic tags. Used for coverage when compiling." },
      { cmd: "--to research", what: "Save to another lane or project." },
      { cmd: "ctx save … --supersedes c:xxxx", what: "Replace an older claim; history is kept." },
      { cmd: "ctx save --paste", what: "Read a ```ctx-claims block off the clipboard." },
    ],
  },
  {
    title: "Reading",
    rows: [
      { cmd: "ctx pack", what: "The compiled context, exactly as your agents get it." },
      { cmd: 'ctx pack --task "<what you are doing>"', what: "Focus it on one task." },
      { cmd: "ctx pack --for dossier | handoff", what: "Longer shapes, for people." },
      { cmd: "ctx pack --out AGENTS.md", what: "Write it; only the marked region is replaced." },
      { cmd: "ctx search <words>", what: "Hybrid search. --all includes deleted." },
      { cmd: "ctx log", what: "Everything recorded. -v for reasons, --all for deleted." },
      { cmd: "ctx show c:xxxx", what: "One claim in full, with its status history." },
      { cmd: "ctx map", what: "The project as a metro map: lines, stations, colours by kind." },
    ],
  },
  {
    title: "Projects",
    rows: [
      { cmd: "ctx init", what: "Wire this repo. Idempotent." },
      { cmd: "ctx onboard", what: "A prompt that fills an existing repo from its own code." },
      { cmd: "ctx list", what: "Every project, its branches and sizes." },
      { cmd: "ctx new <idea>", what: "Start an idea; chats save into it." },
      { cmd: "ctx use <idea>", what: "Switch which idea the chats write to." },
      { cmd: "ctx build <idea> [dir]", what: "Repo + SPEC.md + AGENTS.md + agent wiring." },
      { cmd: "ctx rename <old> <new>", what: "Carry a project's context to a new name." },
    ],
  },
  {
    title: "Documents",
    rows: [
      { cmd: "ctx spec save <file> --to <idea>/research", what: "Store a spec. Versions kept." },
      { cmd: "ctx spec save --paste", what: "Take a ```ctx-spec block from a chat." },
      { cmd: "ctx spec show [name]", what: "Read it. Also written to .ctx/SPEC.md." },
      { cmd: "ctx spec ls", what: "Every document and its size." },
    ],
  },
  {
    title: "Keeping it clean",
    note: "Deleting hides something everywhere and syncs that. Nothing leaves the log.",
    rows: [
      { cmd: "ctx delete c:xxxx", what: "One claim." },
      { cmd: "ctx delete <idea>/<lane>", what: "One branch." },
      { cmd: "ctx delete <idea> --cloud", what: "A whole project; fails unless the cloud agrees." },
      { cmd: "ctx rate c:xxxx up | down", what: "Vote. Feeds the compiler's weighting." },
      { cmd: "ctx refine", what: "Find near-duplicates worth merging. No model involved." },
      { cmd: "ctx verify", what: "Re-hash every record against the log." },
    ],
  },
  {
    title: "Everything else",
    rows: [
      { cmd: "ctx sync", what: "Pull with rebase, push. The only command that needs a network." },
      { cmd: "ctx status", what: "Where am I: store, branch, counts, pending proposals." },
      { cmd: "ctx doctor", what: "What is wired, what is missing, what to run next." },
      { cmd: "ctx branch ls | new | merge | archive", what: "The branch graph." },
      { cmd: "ctx review", what: "Accept or reject claims proposed from another branch." },
      { cmd: "ctx mcp", what: "The MCP server over stdio. Your agent runs this, not you." },
      { cmd: "ctx daemon", what: "Loopback HTTP on 127.0.0.1:7777 for the browser extension." },
      { cmd: "ctx commands", what: "This list, in your terminal." },
    ],
  },
];

export default function Commands() {
  return (
    <>
      <h1>Command reference</h1>
      <p className="lede">
        You are not meant to memorise these. <code>ctx init</code> writes{" "}
        <code>.ctx/commands.md</code> into your repo so your coding agent can run them for you
        while you talk in plain words. This page is for when you want to do it yourself.
      </p>

      {GROUPS.map((g) => (
        <section key={g.title}>
          <h2>{g.title}</h2>
          {g.note && <p>{g.note}</p>}
          <div className="table-wrap">
            <table>
              <tbody>
                {g.rows.map((r) => (
                  <tr key={r.cmd}>
                    <td className="num">{r.cmd}</td>
                    <td className="wrap-cell">{r.what}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </section>
      ))}
    </>
  );
}
