/* eslint-disable @next/next/no-img-element -- This small static brand asset is served directly by the Sites worker. */

type HubuWordmarkProps = {
  className: string;
  decorative?: boolean;
};

export function HubuWordmark({ className, decorative = false }: HubuWordmarkProps) {
  return (
    <img
      className={`hubu-wordmark ${className}`}
      src="/brand/hubu-wordmark.png"
      width="1168"
      height="376"
      alt={decorative ? "" : "Hubu"}
      aria-hidden={decorative || undefined}
      decoding="async"
    />
  );
}
