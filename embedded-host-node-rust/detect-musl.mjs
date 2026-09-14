// Shared musl detection for the build and test scripts.
//
// Mirrors the host's own detection (`compiler-module.ts` via `lib/src/elf.ts`):
// the platform is musl iff the running Node binary's ELF interpreter is
// `ld-musl-*`. This intentionally does NOT use the old `process.report`
// `glibcVersionArray` heuristic — that field is absent in current Node, so it
// classified every glibc host as musl (and broke CI assembly, which then
// disagreed with stock resolution).
import * as fs from 'node:fs';
import * as p from 'node:path';

const PT_INTERP = 3;

function readInterpreter(execPath) {
  let fd;
  try {
    fd = fs.openSync(execPath, 'r');
    const ehdr = Buffer.alloc(64);
    if (fs.readSync(fd, ehdr, 0, 64, 0) !== 64) return null;
    if (
      ehdr[0] !== 0x7f ||
      ehdr[1] !== 0x45 ||
      ehdr[2] !== 0x4c ||
      ehdr[3] !== 0x46
    ) {
      return null;
    }
    const is64 = ehdr[4] === 2;
    const little = ehdr[5] === 1;
    const u16 = (buf, o) => (little ? buf.readUInt16LE(o) : buf.readUInt16BE(o));
    const u32 = (buf, o) => (little ? buf.readUInt32LE(o) : buf.readUInt32BE(o));
    const u64 = (buf, o) =>
      Number(little ? buf.readBigUInt64LE(o) : buf.readBigUInt64BE(o));
    // e_phoff / e_phentsize / e_phnum offsets differ by ELF class.
    const phoff = is64 ? u64(ehdr, 0x20) : u32(ehdr, 0x1c);
    const phentsize = is64 ? u16(ehdr, 0x36) : u16(ehdr, 0x2a);
    const phnum = is64 ? u16(ehdr, 0x38) : u16(ehdr, 0x2c);
    for (let i = 0; i < phnum; i++) {
      const phdr = Buffer.alloc(phentsize);
      fs.readSync(fd, phdr, 0, phentsize, phoff + i * phentsize);
      if (u32(phdr, 0) !== PT_INTERP) continue;
      // p_offset / p_filesz: +8/+32 (64-bit), +4/+16 (32-bit).
      const off = is64 ? u64(phdr, 8) : u32(phdr, 4);
      const len = is64 ? u64(phdr, 32) : u32(phdr, 16);
      const buf = Buffer.alloc(len);
      fs.readSync(fd, buf, 0, len, off);
      return buf.toString('utf8').replace(/\0.*$/, '');
    }
    return null;
  } catch {
    return null;
  } finally {
    if (fd !== undefined) fs.closeSync(fd);
  }
}

export function isMusl() {
  if (process.platform !== 'linux') return false;
  try {
    const interpreter = readInterpreter(process.execPath);
    if (interpreter == null) return false;
    return p.basename(interpreter).startsWith('ld-musl-');
  } catch {
    // Mirror stock: warn and fall back to gnu rather than failing the build.
    console.warn(
      `Warning: Failed to detect linux-musl, fallback to linux-gnu`,
    );
    return false;
  }
}

export function triple() {
  const platform =
    process.platform === 'linux' && isMusl() ? 'linux-musl' : process.platform;
  return `${platform}-${process.arch}`;
}
