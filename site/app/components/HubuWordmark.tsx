/* eslint-disable @next/next/no-img-element -- This static vector brand asset is served directly by the site worker. */

type HubuWordmarkProps = {
  className: string;
  decorative?: boolean;
};

export function HubuWordmark({ className, decorative = false }: HubuWordmarkProps) {
  return (
    <img
      className={`hubu-wordmark ${className}`}
      src="/brand/hubu-wordmark.svg"
      width="1168"
      height="376"
      alt={decorative ? "" : "Hubu"}
      aria-hidden={decorative || undefined}
      decoding="async"
    />
  );
}
