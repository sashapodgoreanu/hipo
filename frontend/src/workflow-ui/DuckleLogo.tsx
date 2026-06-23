// Duckle brand mark: the pipeline-node logo (three connected nodes). Inline SVG so
// it stays crisp at any size with no raster image (and no overflow/scroll). Decorative
// by default - the adjacent "Duckle" wordmark carries the accessible name.
export function DuckleLogo({ size = 24, className }: { size?: number; className?: string }) {
    return (
        <svg
            width={size}
            height={size}
            viewBox="0 0 64 64"
            className={className ? `duckle-logo ${className}` : 'duckle-logo'}
            aria-hidden="true"
            focusable="false"
        >
            <line x1="14" y1="16" x2="32" y2="32" stroke="#EA7E42" strokeWidth="3.4" strokeLinecap="round" />
            <line x1="32" y1="32" x2="50" y2="48" stroke="#EA7E42" strokeWidth="3.4" strokeLinecap="round" />
            <rect x="5" y="7" width="18" height="18" rx="5.5" fill="#F6BA78" />
            <rect x="23" y="23" width="18" height="18" rx="5.5" fill="#EA7E42" />
            <rect x="41" y="39" width="18" height="18" rx="5.5" fill="#D9742F" />
        </svg>
    );
}
