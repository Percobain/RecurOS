/** The RecurOS mark: an R cut out of a disc. Kept as one component so the
 *  header, the favicon route and the hero never drift apart. */
export function Mark({ size = 22, fill = "currentColor" }: { size?: number; fill?: string }) {
  return (
    <svg viewBox="0 0 100 100" width={size} height={size} aria-hidden="true">
      <mask id="recuros-cut">
        <circle cx="50" cy="50" r="49" fill="#fff" />
        <path
          d="M -6 32 C 16 19 44 19 59 33 C 72 45 65 58 46 62 C 33 65 26 78 26 104"
          fill="none"
          stroke="#000"
          strokeWidth="13"
          strokeLinecap="round"
        />
      </mask>
      <circle cx="50" cy="50" r="49" fill={fill} mask="url(#recuros-cut)" />
    </svg>
  );
}
