import * as React from "react";

/** A token broken only where it means to: after each "/", so a path never
 * splits inside a file's name (a content-hashed name is one token). */
function Breakable({ text }: { text: string }) {
  const parts = text.split(/(?<=\/)/);
  return (
    <>
      {parts.map((part, i) => (
        <React.Fragment key={i}>
          {part}
          {i < parts.length - 1 ? <wbr /> : null}
        </React.Fragment>
      ))}
    </>
  );
}

/** A failure's lines to set as they are (headers, a path): mono, one a
 * line. A line breaks at its spaces, or a path at its slashes; never
 * inside a word, so "Cross-Origin-Opener-Policy" stays whole. */
export function FailureLines({ lines }: { lines?: string[] }) {
  if (!lines?.length) return null;
  return (
    // A step smaller on a phone, so a content-hashed file name fits its
    // line whole.
    <ul className="flex flex-col font-mono text-[0.8125rem] leading-5 text-foreground max-sm:text-xs">
      {lines.map(line => (
        // A hanging indent: a value that wraps sits under its name, not
        // where a line of its own would start.
        // A header's wrapped value hangs under its name; a path, which
        // breaks at its slashes, keeps one left edge. A name too long for
        // the line breaks at its edge only then.
        <li
          key={line}
          className={
            line.includes(" ")
              ? "pl-4 -indent-4 [overflow-wrap:anywhere]"
              : "[overflow-wrap:anywhere]"
          }
        >
          {line.split(" ").map((word, i) => (
            <React.Fragment key={i}>
              {i > 0 ? " " : null}
              {/* Chrome breaks at a hyphen: a hyphenated word is kept
                  whole; a path breaks at its slashes. */}
              <span
                className={word.includes("-") ? "whitespace-nowrap" : undefined}
              >
                <Breakable text={word} />
              </span>
            </React.Fragment>
          ))}
        </li>
      ))}
    </ul>
  );
}
