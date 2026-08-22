// Everything the download page shows about what's downloadable is derived
// from the actual latest GitHub release at build time -- OS/arch options,
// binary names, download URLs, checksums file, source tarball. None of it
// is a hardcoded list: release.yml can rename an asset, add or drop an
// arch, and this picks it up on the next build with zero website changes.
// If the API is unreachable at build time, falls back to the last known
// real release shape so the site still builds with accurate (if
// momentarily un-refreshed) data, never invented data.

export interface ReleaseAsset {
  name: string;
  url: string;
}

export interface ArchOption {
  id: string;
  os: 'unix' | 'windows';
  label: string;
  bin: ReleaseAsset;
  lib: ReleaseAsset | null;
}

export interface ReleaseInfo {
  version: string | null;
  archs: ArchOption[];
  checksums: ReleaseAsset | null;
  source: ReleaseAsset | null;
}

const REPO = 'ecnivslabs/olive';
const LATEST_BASE = `https://github.com/${REPO}/releases/latest/download`;

const FALLBACK: ReleaseInfo = {
  version: null,
  archs: [
    { id: 'linux-x86_64', os: 'unix', label: 'Linux, x86_64', bin: { name: 'pit-linux-x86_64', url: `${LATEST_BASE}/pit-linux-x86_64` }, lib: { name: 'libolive_std-linux-x86_64.so', url: `${LATEST_BASE}/libolive_std-linux-x86_64.so` } },
    { id: 'linux-aarch64', os: 'unix', label: 'Linux, ARM64', bin: { name: 'pit-linux-aarch64', url: `${LATEST_BASE}/pit-linux-aarch64` }, lib: { name: 'libolive_std-linux-aarch64.so', url: `${LATEST_BASE}/libolive_std-linux-aarch64.so` } },
    { id: 'macos-x86_64', os: 'unix', label: 'macOS, Intel', bin: { name: 'pit-macos-x86_64', url: `${LATEST_BASE}/pit-macos-x86_64` }, lib: { name: 'libolive_std-macos-x86_64.dylib', url: `${LATEST_BASE}/libolive_std-macos-x86_64.dylib` } },
    { id: 'macos-aarch64', os: 'unix', label: 'macOS, Apple Silicon', bin: { name: 'pit-macos-aarch64', url: `${LATEST_BASE}/pit-macos-aarch64` }, lib: { name: 'libolive_std-macos-aarch64.dylib', url: `${LATEST_BASE}/libolive_std-macos-aarch64.dylib` } },
    { id: 'windows-x86_64', os: 'windows', label: 'Windows, x86_64', bin: { name: 'pit-windows-x86_64.exe', url: `${LATEST_BASE}/pit-windows-x86_64.exe` }, lib: null },
  ],
  checksums: { name: 'checksums.txt', url: `${LATEST_BASE}/checksums.txt` },
  source: { name: 'olive-src.tar.gz', url: `${LATEST_BASE}/olive-src.tar.gz` },
};

function osLabel(os: string): string {
  return { linux: 'Linux', macos: 'macOS', windows: 'Windows' }[os] ?? os;
}

function archLabel(os: string, arch: string): string {
  if (arch === 'x86_64') return os === 'macos' ? 'Intel' : 'x86_64';
  if (arch === 'aarch64') return os === 'macos' ? 'Apple Silicon' : 'ARM64';
  return arch;
}

// pit-{os}-{arch} or pit-{os}-{arch}.exe -- the naming convention release.yml
// actually builds today. A rename of this scheme needs a matching website
// change either way; this at least stops silently missing new os/arch pairs.
const BIN_RE = /^pit-([a-z0-9]+)-([a-z0-9_]+?)(\.exe)?$/;

export async function fetchRelease(): Promise<ReleaseInfo> {
  try {
    const res = await fetch(`https://api.github.com/repos/${REPO}/releases/latest`);
    if (!res.ok) return FALLBACK;
    const data = await res.json();
    const assets: { name: string; browser_download_url: string }[] = Array.isArray(data.assets) ? data.assets : [];

    const archs: ArchOption[] = [];
    for (const asset of assets) {
      const match = asset.name.match(BIN_RE);
      if (!match) continue;
      const [, os, arch] = match;
      const libExt = os === 'linux' ? 'so' : os === 'macos' ? 'dylib' : null;
      const libAsset = libExt ? assets.find((a) => a.name === `libolive_std-${os}-${arch}.${libExt}`) : undefined;

      archs.push({
        id: `${os}-${arch}`,
        os: os === 'windows' ? 'windows' : 'unix',
        label: `${osLabel(os)}, ${archLabel(os, arch)}`,
        bin: { name: asset.name, url: asset.browser_download_url },
        lib: libAsset ? { name: libAsset.name, url: libAsset.browser_download_url } : null,
      });
    }
    if (archs.length === 0) return FALLBACK;

    // Asset order from the API is whatever GitHub happens to return; give
    // the tabs a stable, sensible order instead (this is presentation, not
    // a hardcoded set -- a new os/arch combo still just slots in by rule).
    const osOrder = ['linux', 'macos', 'windows'];
    const archOrder = ['x86_64', 'aarch64'];
    archs.sort((a, b) => {
      const [aOs, aArch] = a.id.split('-');
      const [bOs, bArch] = b.id.split('-');
      const osDiff = osOrder.indexOf(aOs) - osOrder.indexOf(bOs);
      if (osDiff !== 0) return osDiff;
      return archOrder.indexOf(aArch) - archOrder.indexOf(bArch);
    });

    const checksumsAsset = assets.find((a) => a.name === 'checksums.txt');
    const sourceAsset = assets.find((a) => a.name.endsWith('.tar.gz'));

    return {
      version: typeof data.tag_name === 'string' ? data.tag_name : null,
      archs,
      checksums: checksumsAsset ? { name: checksumsAsset.name, url: checksumsAsset.browser_download_url } : null,
      source: sourceAsset ? { name: sourceAsset.name, url: sourceAsset.browser_download_url } : null,
    };
  } catch {
    return FALLBACK;
  }
}
