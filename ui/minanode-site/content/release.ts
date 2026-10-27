import { siteOrigin } from "@/lib/origin";

/** Launch URLs: verify the planned public release at publish time (C69). */
export const release = {
  version: "0.20.0",
  repositoryUrl: "https://github.com/minanode/mina-rust",
  downloadUrl:
    "https://github.com/minanode/mina-rust/releases/download/v0.20.0",
  notesUrl: "https://github.com/minanode/mina-rust/releases/tag/v0.20.0",
};

export function archiveName(arch: string) {
  return `mina-${release.version}-linux-${arch}.tar.gz`;
}

export const releaseLabel = `${release.version} beta for devnet`;

/** The Docker image, pushed to GHCR at release (docs/release/DOCKER.md). */
export const dockerImage = "ghcr.io/minanode/mina-rust";
export const dockerImageRef = `${dockerImage}:${release.version}`;

/** The compose files, served from the site (public/docker/ links to them). */
export const composeBaseUrl = `${siteOrigin}/docker`;

/** Replace with the SHA-256 fingerprint of docs/release/allowed_signers. */
export const releaseSigningFingerprint =
  "SHA256:6h5S0dmb/Ogj3Hkk/E8+BwZZ1FiCsf8QbVvBzGqERdw";

export const releaseSignerUrl = `${siteOrigin}/allowed_signers`;
export const verifyReleaseCommand = `ssh-keygen -Y verify -f allowed_signers \\
  -I release@minanode.org -n file \\
  -s SHA256SUMS.sig < SHA256SUMS`;
