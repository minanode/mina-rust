#!/usr/bin/env python3
"""The Content-Security-Policy of a web node page (ui/minaweb), made from the
build's own origins: build.sh writes it into build-info.json and serve.json,
the deploy recipes into their headers, and the launch gate
(tools/deploy/minaweb/check.py) compares the served header with it.

    python3 csp.py relays LIST                     # the relays, one a line
    python3 csp.py policy --relay MULTIADDR... [--wasm-url URL]
                   [--circuits-url URL] [--connect ORIGIN]...
    python3 csp.py serve-json TEMPLATE POLICY      # serve.json for a build
    python3 csp.py headers TEMPLATE BUILD_INFO     # a host's _headers file

With the policy every page response also gets X-Frame-Options: DENY (for
browsers without frame-ancestors) and a Permissions-Policy that turns off
the powerful features the page never uses.

Every source, and why the page needs it:

  default-src 'none'      anything not listed below is refused
  script-src 'self' 'wasm-unsafe-eval'
                          the page's own scripts, and compiling the WASM;
                          no inline script and no eval
  worker-src 'self' blob: the node's threads: wasm_thread starts each one
                          from a blob: script that imports the glue, and
                          wasm-bindgen's Atomics.waitAsync fallback is a
                          blob: worker too. Only script already running in
                          the page can make a blob: URL
  connect-src 'self' ...  the page's own files, the WASM and circuit host
                          (WASM_URL, CIRCUITS_URL), every relay's
                          signalling origin (from its multiaddr), which also
                          serves its TURN credentials, and --connect origins
  img-src 'self' blob:    the favicon; the WebRTC clean-up in mina-p2p loads
                          a blob: image to make the browser collect closed
                          peer connections
  style-src 'self'        the page's one stylesheet; React sets style
                          properties through the CSSOM, which this allows
  font-src 'self'         the page serves its fonts itself
  (no frame-src)          default-src refuses every frame. The isolated
                          prover's srcdoc iframe isn't fetched, so frame-src
                          doesn't govern it; it inherits this policy, and
                          its one script is block_prover_host.js from 'self'
  base-uri, form-action, object-src, frame-ancestors 'none'
                          no <base>, no form posts (every form is handled in
                          the page), no plugins, and no framing by any page

WebRTC is not governed by CSP (webrtc 'block' would refuse it; Chrome
ignores that directive), so the peers the node connects to are limited by
the node's seed list, not by this policy.
"""

import argparse
import ipaddress
import json
import re
import sys
from pathlib import Path
from urllib.parse import urlsplit

DEFAULT_PORTS = {"http": 80, "https": 443}
X_FRAME_OPTIONS = "DENY"
# Only features Chrome 155 knows: it warns about others on every page load
# (bluetooth is one). No camera, microphone, location, payments, USB, serial,
# HID or MIDI devices, motion sensors or screen capture.
PERMISSIONS_POLICY = ", ".join(
    f"{feature}=()"
    for feature in (
        "accelerometer",
        "camera",
        "display-capture",
        "geolocation",
        "gyroscope",
        "hid",
        "magnetometer",
        "microphone",
        "midi",
        "payment",
        "serial",
        "usb",
    )
)
# Where a host's _headers file takes the build's policy.
PLACEHOLDER = "@CONTENT_SECURITY_POLICY@"


def origin(url):
    """The origin of an absolute http(s) URL, without its default port."""
    parts = urlsplit(url)
    if parts.scheme not in DEFAULT_PORTS or not parts.hostname:
        raise ValueError(f"not an http(s) URL: {url}")
    host = parts.hostname
    if ":" in host:
        host = f"[{host}]"
    port = parts.port
    if port is None or port == DEFAULT_PORTS[parts.scheme]:
        return f"{parts.scheme}://{host}"
    return f"{parts.scheme}://{host}:{port}"


def relay_origin(multiaddr):
    """The origin of a WebRTC relay's HTTP(S) signalling, from its
    multiaddr (/ip4|ip6|dns|dns4|dns6/HOST/tcp/PORT/webrtc/http|https/...)."""
    match = re.fullmatch(
        r"/(ip4|ip6|dns|dns4|dns6)/([^/]+)/tcp/([0-9]+)/webrtc/(https?)/p2p/[^/]+",
        multiaddr,
    )
    if not match:
        raise ValueError(f"not a WebRTC relay multiaddr: {multiaddr}")
    kind, host, port, scheme = match.groups()
    if kind == "ip6":
        # A source expression has no IPv6 form: Chrome drops [::1] as invalid.
        raise ValueError(f"a Content-Security-Policy cannot name an IPv6 relay; give it a DNS name: {multiaddr}")
    if kind == "ip4":
        host = str(ipaddress.IPv4Address(host))
    return origin(f"{scheme}://{host}:{port}/")


def relay_list(text):
    """The relays of a build's RELAY setting: WebRTC multiaddrs separated by
    commas or white space, at least one, none twice and no peer id twice,
    each with a signalling origin a policy can name."""
    relays = [r for r in re.split(r"[\s,]+", text) if r]
    if not relays:
        raise ValueError("no relay")
    for relay in relays:
        relay_origin(relay)
    if len(set(relays)) != len(relays):
        raise ValueError("a relay is listed twice")
    peers = [r.rsplit("/p2p/", 1)[1] for r in relays]
    if len(set(peers)) != len(peers):
        raise ValueError("two relays have one peer id")
    return relays


def policy(relays, wasm_url=None, circuits_url=None, connect=()):
    connect_src = ["'self'"]
    for source in [
        *(origin(u) for u in (wasm_url, circuits_url) if u and "://" in u),
        *(relay_origin(r) for r in relays),
        *(origin(c) for c in connect),
    ]:
        if source not in connect_src:
            connect_src.append(source)
    directives = [
        ("default-src", ["'none'"]),
        ("script-src", ["'self'", "'wasm-unsafe-eval'"]),
        ("worker-src", ["'self'", "blob:"]),
        ("connect-src", connect_src),
        ("img-src", ["'self'", "blob:"]),
        ("style-src", ["'self'"]),
        ("font-src", ["'self'"]),
        ("base-uri", ["'none'"]),
        ("form-action", ["'none'"]),
        ("object-src", ["'none'"]),
        ("frame-ancestors", ["'none'"]),
    ]
    return "; ".join(f"{name} {' '.join(values)}" for name, values in directives)


def parse(text):
    """A policy's directives, {name: [sources]}; a repeated directive is an
    error here (browsers ignore all but the first)."""
    directives = {}
    for part in text.split(";"):
        tokens = part.split()
        if not tokens:
            continue
        name = tokens[0].lower()
        if name in directives:
            raise ValueError(f"directive {name} repeated")
        directives[name] = tokens[1:]
    return directives


def page_headers(csp):
    """The headers this policy comes with, for every page response."""
    return [
        ("Content-Security-Policy", csp),
        ("X-Frame-Options", X_FRAME_OPTIONS),
        ("Permissions-Policy", PERMISSIONS_POLICY),
    ]


def serve_json(template, csp):
    """serve.json for a build: the template's, with the policy's headers on
    every path. No policy (the Angular page): the template's only."""
    config = json.loads(Path(template).read_text())
    if csp:
        for rule in config["headers"]:
            if rule["source"] == "**":
                rule["headers"] += [{"key": k, "value": v} for k, v in page_headers(csp)]
                break
        else:
            raise ValueError(f"{template} has no rule for every path")
    return json.dumps(config, indent=2) + "\n"


def headers_file(template, build_info):
    """A static host's _headers, with the build's policy in it."""
    text = Path(template).read_text()
    csp = json.loads(Path(build_info).read_text()).get("contentSecurityPolicy")
    if not csp:
        raise ValueError(f"{build_info} has no contentSecurityPolicy: not a minaweb build")
    if text.count(PLACEHOLDER) != 1:
        raise ValueError(f"{template} must hold {PLACEHOLDER} once")
    return text.replace(PLACEHOLDER, csp)


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = p.add_subparsers(dest="command", required=True)
    relays = sub.add_parser("relays")
    relays.add_argument("list")
    make = sub.add_parser("policy")
    make.add_argument("--relay", action="append", default=[], required=True)
    make.add_argument("--wasm-url")
    make.add_argument("--circuits-url")
    make.add_argument("--connect", action="append", default=[])
    serve = sub.add_parser("serve-json")
    serve.add_argument("template")
    serve.add_argument("policy")
    host = sub.add_parser("headers")
    host.add_argument("template")
    host.add_argument("build_info")
    args = p.parse_args()
    try:
        if args.command == "relays":
            print("\n".join(relay_list(args.list)))
        elif args.command == "policy":
            print(policy(args.relay, args.wasm_url, args.circuits_url, args.connect))
        elif args.command == "serve-json":
            sys.stdout.write(serve_json(args.template, args.policy))
        else:
            sys.stdout.write(headers_file(args.template, args.build_info))
    except ValueError as error:
        sys.exit(f"csp.py: {error}")


if __name__ == "__main__":
    main()
