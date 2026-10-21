import { describe, expect, it } from "vitest";
import { diskPath, fileManagerOf, pictureBase } from "../paths";

describe("where a folder is shown", () => {
  it("is written with backslashes after a drive or a folder of Windows", () => {
    expect(diskPath("D:\\", "Photos\\Trip")).toBe("D:\\Photos\\Trip");
    expect(diskPath("C:\\Mounts\\Disk1\\", "Photos")).toBe("C:\\Mounts\\Disk1\\Photos");
    expect(diskPath("D:\\", "")).toBe("D:\\");
    expect(diskPath("", "Photos")).toBe("\\Photos");
  });

  it("is written with slashes after a mount point of macOS or Linux", () => {
    expect(diskPath("/Volumes/Photos/", "2026\\Trip to Paris")).toBe("/Volumes/Photos/2026/Trip to Paris");
    expect(diskPath("/", "Users\\me\\Pictures")).toBe("/Users/me/Pictures");
    expect(diskPath("/", "")).toBe("/");
    expect(diskPath("/Volumes/Photos", "")).toBe("/Volumes/Photos/");
  });

  it("follows the disk, not the system: a disk last seen on Windows keeps its backslashes", () => {
    expect(diskPath("E:\\", "Archive\\2019")).toBe("E:\\Archive\\2019");
  });
});

describe("where pictures are asked at", () => {
  it("is Tauri's http address on Windows and Android, its own scheme elsewhere", () => {
    const edge = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130 Edg/130";
    const safari = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko)";
    const gtk = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17 Safari/605.1.15";
    const android = "Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130 Mobile";
    expect(pictureBase(edge)).toBe("http://shoebox.localhost");
    expect(pictureBase(android)).toBe("http://shoebox.localhost");
    expect(pictureBase(safari)).toBe("shoebox://localhost");
    expect(pictureBase(gtk)).toBe("shoebox://localhost");
  });
});

describe("what the system calls its file manager", () => {
  it("is Explorer, Finder or the file manager", () => {
    expect(fileManagerOf("Mozilla/5.0 (Windows NT 10.0; Win64; x64) Chrome/130").name).toBe("Explorer");
    expect(fileManagerOf("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15").name).toBe("Finder");
    expect(fileManagerOf("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/605.1.15").name).toBe("the file manager");
    // Short enough for a button.
    expect(fileManagerOf("Mozilla/5.0 (X11; Linux x86_64)").button).toBe("Files");
    expect(fileManagerOf("Mozilla/5.0 (Windows NT 10.0)").button).toBe("Explorer");
  });
});
