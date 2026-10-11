import { copyFile, mkdir } from 'node:fs/promises';
import { existsSync } from 'node:fs';
import { resolve } from 'node:path';
import { execFileSync } from 'node:child_process';
import process from 'node:process';

const root = resolve(import.meta.dirname, '../..');
const target = process.env.TAURI_ENV_TARGET_TRIPLE || process.env.RUST_TARGET || process.env.TARGET || execFileSync('rustc', ['-vV'], { encoding: 'utf8' }).match(/^host: (.+)$/m)?.[1];
if (!target) throw new Error('Rust target triple unavailable');

const cargoArgs = ['build', '-p', 'buglogin-migration', '--bin', 'buglogin-migrate', '--release'];
if (process.env.TAURI_ENV_TARGET_TRIPLE || process.env.RUST_TARGET || process.env.TARGET) cargoArgs.push('--target', target);
execFileSync('cargo', cargoArgs, { cwd: root, stdio: 'inherit' });

const targetRoot = process.env.TAURI_ENV_TARGET_TRIPLE || process.env.RUST_TARGET || process.env.TARGET ? resolve(root, 'target', target, 'release') : resolve(root, 'target', 'release');
const extension = process.platform === 'win32' ? '.exe' : '';
const source = resolve(targetRoot, `buglogin-migrate${extension}`);
if (!existsSync(source)) throw new Error(`Migration binary missing: ${source}`);

const destinationDir = resolve(import.meta.dirname, 'src-tauri', 'binaries');
await mkdir(destinationDir, { recursive: true });
await copyFile(source, resolve(destinationDir, `buglogin-migrate-${target}${extension}`));
