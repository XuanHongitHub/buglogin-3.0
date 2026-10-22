const fs = require('fs');
const path = require('path');

function getHandlers() {
  const libRs = fs.readFileSync('apps/desktop/src-tauri/src/lib.rs', 'utf8');
  const start = libRs.indexOf('tauri::generate_handler![');
  const end = libRs.indexOf('])', start);
  const block = libRs.substring(start, end);
  const handlers = new Set();
  for (const line of block.split('\n')) {
    const trimmed = line.trim().replace(',', '');
    if (trimmed && !trimmed.startsWith('//') && !trimmed.startsWith('tauri::')) {
      const parts = trimmed.split('::');
      handlers.add(parts[parts.length - 1]);
    }
  }
  return handlers;
}

const handlers = getHandlers();
const globFiles = [];
function walk(dir) {
  for (const f of fs.readdirSync(dir)) {
    const full = path.join(dir, f);
    if (fs.statSync(full).isDirectory()) walk(full);
    else if (full.endsWith('.ts') || full.endsWith('.tsx')) globFiles.push(full);
  }
}
walk('src');

const map = {};
for (const file of globFiles) {
  const text = fs.readFileSync(file, 'utf8');
  const regex = /invoke(?:<[^>]+>)?\(\s*["']([^"']+)["']/g;
  let match;
  while ((match = regex.exec(text)) !== null) {
    const cmd = match[1];
    if (!handlers.has(cmd)) {
      if (!map[cmd]) map[cmd] = [];
      const relPath = file.replace(/\\/g, '/');
      if (!map[cmd].includes(relPath)) {
        map[cmd].push(relPath);
      }
    }
  }
}

const sorted = Object.keys(map).sort();
console.log('Total unique missing commands:', sorted.length);
for (const cmd of sorted) {
  console.log(`${cmd} | ${map[cmd].length} file(s) | ${map[cmd].join(', ')}`);
}
