export function Loading() {
  return <div class="loading" aria-live="polite" aria-busy="true">Loading…</div>;
}

interface ErrorBannerProps {
  code: string;
  message: string;
  spec?: string;
  anchor?: string;
}

export function ErrorBanner({ code, message, spec, anchor }: ErrorBannerProps) {
  let text: string;
  if (code === 'spec_not_indexed') {
    text = `${spec ?? 'This spec'} is not part of this index`;
  } else if (code === 'not_found') {
    text = `No section ${anchor ?? ''} in ${spec ?? ''}`;
  } else {
    text = message;
  }
  return (
    <div class="error-banner" role="alert">
      {text}
      <p style={{ marginTop: 'var(--space-2)' }}>
        <a href="#/">Return to landing page</a>
      </p>
    </div>
  );
}
