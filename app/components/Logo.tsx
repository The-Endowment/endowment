import { LOGO_GRID } from "@/lib/site";

const cells = LOGO_GRID.flatMap((row, y) =>
  row.split("").flatMap((ch, x) => (ch === " " ? [] : [{ x, y, column: ch === "#" }])),
);

/** The dot-grid temple, for large sizes. */
export function DotLogo({ className, label }: { className?: string; label?: string }) {
  return (
    <svg
      className={className}
      viewBox="-0.5 -0.5 25 15"
      role={label ? "img" : undefined}
      aria-label={label}
      aria-hidden={label ? undefined : true}
    >
      {cells.map(({ x, y, column }) => (
        <circle key={`${x}-${y}`} cx={x} cy={y} r={0.3} className={column ? "logo-column" : "logo-building"} />
      ))}
    </svg>
  );
}

const BUILDING = "M12 0h1v1h-1zM9 1h7v1h-7zM6 2h13v1h-13zM4 3h17v1h-17zM0 4h25v1h-25zM0 14h25v1h-25z";
const COLUMNS =
  "M3 6h1v1h-1zM9 6h1v1h-1zM15 6h1v1h-1zM21 6h1v1h-1zM2 7h3v5h-3zM8 7h3v5h-3zM14 7h3v5h-3zM20 7h3v5h-3z" +
  "M1 12h5v1h-5zM7 12h5v1h-5zM13 12h5v1h-5zM19 12h5v1h-5z" +
  "M1 13h2v1h-2zM4 13h2v1h-2zM7 13h2v1h-2zM10 13h2v1h-2zM13 13h2v1h-2zM16 13h2v1h-2zM19 13h2v1h-2zM22 13h2v1h-2z";

/** The solid version, for small sizes where dots blur. */
export function SolidLogo({ className }: { className?: string }) {
  return (
    <svg className={className} viewBox="0 0 25 15" aria-hidden="true" shapeRendering="crispEdges">
      <path d={BUILDING} className="logo-building" />
      <path d={COLUMNS} className="logo-column" />
    </svg>
  );
}
