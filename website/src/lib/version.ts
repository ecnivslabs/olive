// Latest-version lookup for the download page. Falls back to a plain
// "latest" label rather than a guessed version number if the API is
// unreachable at build time -- every download link on the page uses
// GitHub's version-agnostic /releases/latest/download/ path regardless,
// so a stale or missing version string never breaks a link, only the
// cosmetic "current release: vX" line.
export async function fetchLatestVersion(): Promise<string | null> {
  try {
    const res = await fetch('https://api.github.com/repos/ecnivslabs/olive/releases/latest');
    if (!res.ok) return null;
    const data = await res.json();
    return typeof data.tag_name === 'string' ? data.tag_name : null;
  } catch {
    return null;
  }
}
