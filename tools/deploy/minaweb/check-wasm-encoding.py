#!/usr/bin/env python3
"""What the asset host sends for a WASM uploaded as brotli (release.py,
WASM_ENCODING=br), to each kind of client: the answer's own Content-Encoding,
which must be one the client accepts, then the body decoded by that encoding
alone, whose SHA-256 must be the pinned WASM's. Nothing here decodes on its
own: http.client hands over the bytes as they were sent.

    check-wasm-encoding.py URL SHA256

Run it on a throwaway object first (MINAWEB.md), then deploy.sh runs it on
each release before the Pages deploy. Exits non-zero on the first failure.
"""
import gzip
import hashlib
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


def decode(encoding, body):
    if encoding == "identity":
        return body
    if encoding == "gzip":
        return gzip.decompress(body)
    if encoding == "br":
        return subprocess.run(["node", "-e", BROTLI], input=body, check=True,
                              capture_output=True).stdout
    raise AssertionError(f"no decoder for {encoding}")


def check(url, pinned, fetch=get):
    """One line per client; raises AssertionError on the first failure."""
    lines = []
    for client, accept, allowed in CLIENTS:
        status, headers, body = fetch(url, accept)
        encoding = (headers.get("Content-Encoding") or "identity").strip().lower()
        prefix = f"{client}: HTTP {status}, Content-Encoding {encoding}, {len(body)} bytes"
        assert status == 200, f"{prefix}: not 200"
        assert encoding in allowed, f"{prefix}: not one of {sorted(allowed)}"
        assert headers.get("Access-Control-Allow-Origin") == ORIGIN, f"{prefix}: no CORS for {ORIGIN}"
        assert (headers.get("Content-Type") or "").split(";")[0].strip() == "application/wasm", \
            f"{prefix}: not application/wasm"
        got = hashlib.sha256(decode(encoding, body)).hexdigest()
        assert got == pinned, f"{prefix}: decodes to {got}, not the pinned {pinned}"
        lines.append(f"{prefix}: decodes to the pinned WASM")
    return lines


def main():
    if len(sys.argv) != 3:
        raise SystemExit(__doc__)
    try:
        for line in check(sys.argv[1], sys.argv[2]):
            print(line)
    except AssertionError as error:
        raise SystemExit(f"check-wasm-encoding: {error}") from None


if __name__ == "__main__":
    main()
