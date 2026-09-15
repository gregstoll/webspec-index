interface Props {
  message?: string;
}

export function NotFound({ message }: Props) {
  return (
    <div class="page">
      <div class="error-banner">
        <strong>Not found.</strong>{message ? ` ${message}` : ' The section or page you requested does not exist.'}
      </div>
      <p>
        <a href="#/">Return to landing page</a>
      </p>
    </div>
  );
}
