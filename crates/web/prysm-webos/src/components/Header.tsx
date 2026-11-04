export default function Header({
  subtitle = 'TV capture',
}: {
  subtitle?: string;
}) {
  return (
    <header className="header">
      <div className="brand">
        <svg
          className="logo"
          viewBox="0 0 32 32"
          fill="none"
          aria-hidden="true"
        >
          <path d="m16 3 13 24H3L16 3Z" stroke="currentColor" strokeWidth="2" />
          <path
            d="m16 3 4 24M3 27l15-12 11 12"
            stroke="currentColor"
            strokeWidth="1.5"
          />
        </svg>
        <h1>
          prysm<span className="brand-dot">.</span>
        </h1>
        <p>{subtitle}</p>
      </div>
    </header>
  );
}
