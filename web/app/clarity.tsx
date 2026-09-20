"use client";

import { useEffect } from "react";

/** Microsoft Clarity, started once on the client.
 *
 *  The project id comes from NEXT_PUBLIC_CLARITY_PROJECT_ID. With no id set
 *  this renders nothing and loads nothing, so a local `npm run dev` and a
 *  fork without the variable are not quietly reporting to someone else's
 *  dashboard. Clarity records sessions, so it only ever runs in production.
 */
export function Clarity() {
  const projectId = process.env.NEXT_PUBLIC_CLARITY_PROJECT_ID;

  useEffect(() => {
    if (!projectId || process.env.NODE_ENV !== "production") return;
    let cancelled = false;
    import("@microsoft/clarity")
      .then((mod) => {
        if (!cancelled) mod.default.init(projectId);
      })
      .catch(() => {
        // Analytics is never worth breaking a page over.
      });
    return () => {
      cancelled = true;
    };
  }, [projectId]);

  return null;
}
