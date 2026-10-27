#!/usr/bin/env bash
set -euo pipefail
# Administrator step for the headless Chrome used by the web recording harness.
source /etc/os-release
case ${VERSION_ID:-} in
    22.04) audio=libasound2; atk=libatk1.0-0; bridge=libatk-bridge2.0-0; cups=libcups2 ;;
    24.04) audio=libasound2t64; atk=libatk1.0-0t64; bridge=libatk-bridge2.0-0t64; cups=libcups2t64 ;;
    *) echo 'This script supports Ubuntu 22.04 and 24.04; install Chrome runtime libraries for your OS.' >&2; exit 2 ;;
esac
sudo apt update
sudo apt install -y libnss3 "$atk" "$bridge" "$cups" libdrm2 \
    libxkbcommon0 libxcomposite1 libxdamage1 libxfixes3 libxrandr2 libgbm1 \
    "$audio" libxi6 libxrender1
