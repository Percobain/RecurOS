import { ImageResponse } from "next/og";

export const size = { width: 32, height: 32 };
export const contentType = "image/png";

/** The mark, rendered at favicon size. Kept in sync with lib/mark.tsx. */
export default function Icon() {
  return new ImageResponse(
    (
      <div
        style={{
          width: "100%",
          height: "100%",
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
          background: "#0b0b0c",
        }}
      >
        <svg viewBox="0 0 100 100" width="28" height="28">
          <mask id="c">
            <circle cx="50" cy="50" r="49" fill="#fff" />
            <path
              d="M -6 32 C 16 19 44 19 59 33 C 72 45 65 58 46 62 C 33 65 26 78 26 104"
              fill="none"
              stroke="#000"
              strokeWidth="13"
              strokeLinecap="round"
            />
          </mask>
          <circle cx="50" cy="50" r="49" fill="#ededef" mask="url(#c)" />
        </svg>
      </div>
    ),
    size
  );
}
