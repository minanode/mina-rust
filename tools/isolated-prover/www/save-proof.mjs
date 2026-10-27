// The headless runner installs saveProof before page scripts run. A person
// opening the standalone page instead gets downloadable proof files.
window.saveProof ??= async (n, bytes) => {
  const link = document.createElement("a");
  link.download = `proof-${n}.binprot`;
  link.href = URL.createObjectURL(new Blob([new Uint8Array(bytes)]));
  link.textContent = `Download proof ${n + 1} for native verification`;
  document.body.append(link, document.createElement("br"));
};
