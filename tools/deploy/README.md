# Deploying the public web node

The kit for the devnet web node at https://minaweb.org (W1): the relay it joins
through, on a VPS, and the static site. Plain files; `<...>` are placeholders,
and no secret is kept here.

- [`relay/`](relay/README.md): the relay node (systemd), Caddy (TLS for its
  WebRTC signalling), coturn (TURN), the firewall, and the runbook: provision,
  install, check, upgrade, roll back.
- [`site/`](site/README.md): the public build, and the headers and settings for
  Cloudflare Pages and R2, Netlify, or Caddy.

The design, the test results, the steps on the VPS and the costs: private
implementation notes.
