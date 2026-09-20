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

## Analytics

Vercel Analytics needs nothing: it is on for the project in the Vercel
dashboard and `<Analytics />` in the layout does the rest.

Microsoft Clarity (session replay and heatmaps) reads its project id from
`NEXT_PUBLIC_CLARITY_PROJECT_ID`. Set it in Vercel under Settings ->
Environment Variables, for Production only. With no id set, or outside a
production build, nothing loads and nothing is recorded, so `npm run dev` and
forks stay silent instead of reporting into someone else's dashboard.

## Deploy on Vercel

Import the repository, then set **Root Directory** to `web`. Vercel detects
Next.js and needs no other configuration: there is no server code, no
environment variable and no database.
