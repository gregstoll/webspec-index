export function SectionNumber({ number }: { number?: string }) {
  if (!number) return null;
  return <span class="section-number">{number}</span>;
}
