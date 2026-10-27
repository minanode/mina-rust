/** A command, flag or file name inside a sentence. Put real spaces around
 * it in the text, not a margin: a margin shows as a stray space before a
 * full stop, and text that touches it can't wrap there. */
export function Code({ children }: { children: React.ReactNode }) {
  return (
    <code className="font-mono whitespace-nowrap text-foreground">
      {children}
    </code>
  );
}
