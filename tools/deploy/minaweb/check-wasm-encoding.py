#!/usr/bin/env python3
"""What the asset host sends for a WASM uploaded as brotli (release.py,
WASM_ENCODING=br), to each kind of client: the answer's own Content-Encoding,
which must be one the client accepts, then the body decoded by that encoding
alone, whose SHA-256 must be the pinned WASM's. Nothing here decodes on its
own: http.client hands over the bytes as they were sent.

    check-wasm-encoding.py URL SHA256 [ENCODED_SHA256]

With ENCODED_SHA256, the SHA-256 of the uploaded brotli file, a brotli answer
must be that file byte for byte: a decoder may ignore bytes after the end of
the stream, and a browser's may not.

Every check raises and exits non-zero, also under `python -O`; deploy.sh
runs it with `python3 -I`, which ignores PYTHON* variables. Run it on a
throwaway object first, twice: the edge may answer a cache miss
and a hit (cf-cache-status) differently. deploy.sh then runs it on each
release before the Pages deploy. Exits non-zero on the first failure.
"""
import gzip
import hashlib
import re
import subprocess
import sys
from http.client import HTTPConnection, HTTPSConnection
from urllib.parse import urlsplit

# Cloudflare's Browser Integrity Check refuses urllib's default User-Agent.
USER_AGENT = "minaweb-check/1"
ORIGIN = "https://minaweb.org"

# (client, its Accept-Encoding or None for no header, the encodings it may
# get). A browser must get the brotli file as it is. A client without the
# header accepts any coding (RFC 9110, 12.5.3), so it may get brotli too.
CLIENTS = [
    ("Chrome", "gzip, deflate, br, zstd", {"br"}),
    ("gzip only", "gzip", {"gzip", "identity"}),
    ("identity", "identity", {"identity"}),
    ("no Accept-Encoding", None, {"br", "gzip", "identity"}),
]

BROTLI = ('const z = require("zlib"); const c = [];'
          'process.stdin.on("data", d => c.push(d));'
          'process.stdin.on("end", () => process.stdout.write(z.brotliDecompressSync(Buffer.concat(c))));')


def get(url, accept):
    """The answer to one GET, its body as it was sent. http.client, not
    urllib: without an Accept-Encoding of ours, http.client would send
    `Accept-Encoding: identity`, and this must be able to send none."""
    parts = urlsplit(url)
    connection = HTTPSConnection if parts.scheme == "https" else HTTPConnection
    conn = connection(parts.netloc, timeout=60)
    try:
        conn.putrequest("GET", parts.path + (f"?{parts.query}" if parts.query else ""),
                        skip_accept_encoding=True)
        conn.putheader("User-Agent", USER_AGENT)
        conn.putheader("Origin", ORIGIN)
        if accept is not None:
            conn.putheader("Accept-Encoding", accept)
        conn.endheaders()
        response = conn.getresponse()
        return response.status, response.headers, response.read()
    finally:
        conn.close()


# An HTTP token (RFC 9110, 5.6.2): a coding name has no other characters.
TOKEN = re.compile(r"[!#$%&'*+.^_`|~0-9A-Za-z-]+")


class CheckFailed(Exception):
    """An answer the asset host must not give. Raised, never asserted:
    `python -O` (PYTHONOPTIMIZE) removes every assert."""


def require(ok, message):
    if not ok:
        raise CheckFailed(message)


def fields(headers, name):
    """Every field of `name` as sent. A client joins repeated fields into one
    list, so a check of the first alone could pass an answer a browser then
    fails to decode."""
    return [value.strip() for value in headers.get_all(name) or []]


def one_field(headers, name, prefix):
    values = fields(headers, name)
    require(len(values) == 1, f"{prefix}: {len(values)} {name} fields, not one")
    return values[0]


def content_coding(headers, prefix):
    """The answer's one content coding: `identity` without the header,
    else the single coding of its single field."""
    values = fields(headers, "Content-Encoding")
    require(len(values) <= 1, f"{prefix}: {len(values)} Content-Encoding fields")
    if not values:
        return "identity"
    # Every comma-separated element counts, empty ones too: `br,` is not
    # `br` to a browser, which hands over the bytes still encoded.
    elements = [c.strip() for c in values[0].split(",")]
    require(len(elements) == 1 and TOKEN.fullmatch(elements[0]),
            f"{prefix}: Content-Encoding {values[0]!r} is not one coding")
    return elements[0].lower()


def decode(encoding, body, prefix):
    try:
        if encoding == "identity":
            return body
        if encoding == "gzip":
            return gzip.decompress(body)
        if encoding == "br":
            return subprocess.run(["node", "-e", BROTLI], input=body, check=True,
                                  capture_output=True).stdout
    except (OSError, EOFError, subprocess.CalledProcessError) as error:
        raise CheckFailed(f"{prefix}: doesn't decode as {encoding}: {error}") from None
    raise CheckFailed(f"{prefix}: no decoder for {encoding}")


def check(url, pinned, encoded=None, fetch=get):
    """One line per client; raises CheckFailed on the first failure."""
    lines = []
    for client, accept, allowed in CLIENTS:
        status, headers, body = fetch(url, accept)
        shown = ", ".join(fields(headers, "Content-Encoding")) or "none"
        cache = headers.get("cf-cache-status")
        prefix = (f"{client}: HTTP {status}, Content-Encoding {shown}, {len(body)} bytes"
                  + (f", cf-cache-status {cache}" if cache else ""))
        require(status == 200, f"{prefix}: not 200")
        encoding = content_coding(headers, prefix)
        require(encoding in allowed, f"{prefix}: {encoding} is not one of {sorted(allowed)}")
        require(one_field(headers, "Access-Control-Allow-Origin", prefix) == ORIGIN,
                f"{prefix}: no CORS for {ORIGIN}")
        mime = one_field(headers, "Content-Type", prefix).split(";")[0].strip().lower()
        require(mime == "application/wasm", f"{prefix}: {mime}, not application/wasm")
        if encoding == "br" and encoded:
            sent = hashlib.sha256(body).hexdigest()
            require(sent == encoded, f"{prefix}: not the uploaded brotli file ({sent})")
        got = hashlib.sha256(decode(encoding, body, prefix)).hexdigest()
        require(got == pinned, f"{prefix}: decodes to {got}, not the pinned {pinned}")
        lines.append(f"{prefix}: decodes to the pinned WASM")
    return lines


def main():
    if len(sys.argv) not in (3, 4):
        raise SystemExit(__doc__)
    try:
        for line in check(sys.argv[1], sys.argv[2], sys.argv[3] if len(sys.argv) == 4 else None):
            print(line)
    except CheckFailed as error:
        raise SystemExit(f"check-wasm-encoding: {error}") from None


if __name__ == "__main__":
    main()
