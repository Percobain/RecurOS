# recuros.dev

The RecurOS website: landing page, documentation and releases. Next.js App
Router, no UI framework, every page statically rendered.

## The numbers are measured, not written

Everything the site claims about tokens and milliseconds comes from
`data/benchmarks.json`, which is produced by `scripts/bench.py` in the repo
root against RecurOS's own store:

```sh
cd ..                      # repo root, with `ctx` on your PATH
python scripts/bench.py    # rewrites web/data/benchmarks.json
```

Re-run it whenever the store or the packer changes, and commit the JSON. No
page hard-codes a figure; if you find one, it is a bug.

## Develop

```sh
npm install
npm run dev     # http://localhost:3000
npm run build
```

## Deploy on Vercel

Import the repository, then set **Root Directory** to `web`. Vercel detects
Next.js and needs no other configuration: there is no server code, no
environment variable and no database.
