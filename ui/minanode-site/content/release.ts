import { siteOrigin } from "@/lib/origin";

/** Packaging candidate; publication is a separate owner action. */
export const candidateVersion = "0.22.1";

/**
 * Published release only. Keep version, download URLs and Docker digest together.
 * At publication follow docs/release/RELEASING.md, "Published site switch".
 * RELEASE_FINAL validates the signer; it does not publish or select a candidate.
 */
export const release = {
  version: "0.22.0",
  repositoryUrl: "https://github.com/minanode/mina-rust",
  downloadUrl:
    "https://github.com/minanode/mina-rust/releases/download/v0.22.0",
  notesUrl: "https://github.com/minanode/mina-rust/releases/tag/v0.22.0",
  /** The Docker image index digest, as the release notes list it. */
  dockerDigest:
    "sha256:1df8bf7baddb0dca947fb46ddd603b83cdf3a1588dc8af7f9eda1103c27d1729",
};

/** The web node's release, 0.22.0: minaweb.org and the web archive. It is
 * versioned apart from the native release above; the archive is a 0.22.0
 * release asset. */
export const webRelease = {
  version: "0.22.0",
  downloadUrl:
    "https://github.com/minanode/mina-rust/releases/download/v0.22.0",
};

export function archiveName(arch: string) {
  return `mina-${release.version}-linux-${arch}.tar.gz`;
}

export const releaseLabel = `${release.version} beta for devnet`;

/** The Docker image, pushed to GHCR at release (docs/release/DOCKER.md). */
export const dockerImage = "ghcr.io/minanode/mina-rust";
export const dockerImageRef = `${dockerImage}:${release.version}`;
export const dockerImagePinned = `${dockerImageRef}@${release.dockerDigest}`;

/** The compose files, served from the site (public/docker/ links to them). */
export const composeBaseUrl = `${siteOrigin}/docker`;

/** Replace with the SHA-256 fingerprint of docs/release/allowed_signers. */
export const releaseSigningFingerprint =
  "SHA256:6h5S0dmb/Ogj3Hkk/E8+BwZZ1FiCsf8QbVvBzGqERdw";

export const releaseSignerUrl = `${siteOrigin}/allowed_signers`;
export const verifyReleaseCommand = `ssh-keygen -Y verify -f allowed_signers \\
  -I release@minanode.org -n file \\
  -s SHA256SUMS.sig < SHA256SUMS`;
