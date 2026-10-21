// Where things are on disk, and what this system calls them.
//
// A catalog folder (`relPath`) is kept with backslashes between its names on
// every system, so one catalog reads the same everywhere. The place it is
// shown at is written the way its disk writes paths.

/** A catalog folder's place on its disk, as the disk writes paths: with `/`
 *  after a mount point of macOS or Linux (`/Volumes/Photos/`), `\` after a
 *  drive or a folder of Windows (`D:\`). The mount tells, not the system the
 *  window runs on: an unplugged disk is shown where it was last seen, and a
 *  catalog made on one system is opened on others. */
export function diskPath(mount: string, relPath: string): string {
  const slash = mount.startsWith("/");
  const base = mount.replace(/[\\/]$/, "");
  return `${base}${slash ? "/" : "\\"}${slash ? relPath.split("\\").join("/") : relPath}`;
}

/** The base of the addresses pictures are asked at (`thumbUrl`...). Tauri serves
 *  Shoebox's own pictures as `http://shoebox.localhost/...` on Windows and
 *  Android, as `shoebox://localhost/...` elsewhere (macOS, Linux). */
export function pictureBase(userAgent: string): string {
  return /Windows|Android/i.test(userAgent) ? "http://shoebox.localhost" : "shoebox://localhost";
}

/** What this system calls the program that shows folders and files. `name` fits
 *  "Show in ...", `button` is short enough for a button. */
export function fileManagerOf(userAgent: string): { name: string; button: string } {
  if (/Windows/i.test(userAgent)) return { name: "Explorer", button: "Explorer" };
  if (/Macintosh|Mac OS X/i.test(userAgent)) return { name: "Finder", button: "Finder" };
  return { name: "the file manager", button: "Files" };
}

export const FILE_MANAGER = fileManagerOf(navigator.userAgent);
