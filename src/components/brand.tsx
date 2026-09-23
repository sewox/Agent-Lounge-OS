export const LOGO_SRC = "/logo.png";

export function BrandMark({
  size = 22,
  className = "",
}: {
  size?: number;
  className?: string;
}) {
  return (
    <img
      src={LOGO_SRC}
      alt=""
      width={size}
      height={size}
      className={`shrink-0 rounded-sm object-cover ${className}`}
      draggable={false}
    />
  );
}
