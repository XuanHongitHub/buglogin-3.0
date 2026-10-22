const fs = require('fs');
const path = require('path');

function getFiles(dir, exts) {
  let res = [];
  try {
    for (const item of fs.readdirSync(dir, { withFileTypes: true })) {
      const full = path.join(dir, item.name);
      if (item.isDirectory()) {
        if (item.name !== 'node_modules' && item.name !== '.git' && item.name !== 'dist' && item.name !== '.tmp') {
          res = res.concat(getFiles(full, exts));
        }
      } else if (exts.some(ext => item.name.endsWith(ext))) {
        res.push(full);
      }
    }
  } catch (e) {}
  return res;
}

const frontendFiles = [
  ...getFiles('src', ['.ts', '.tsx']),
  ...getFiles('apps/desktop/src', ['.ts', '.tsx'])
];

const invokeRegex = /invoke(?:<[^>]*>)?\(\s*["']([a-zA-Z0-9_]+)["']/g;
const invokesMap = new Map();

for (const file of frontendFiles) {
  const content = fs.readFileSync(file, 'utf8');
  let match;
  while ((match = invokeRegex.exec(content)) !== null) {
    const cmd = match[1];
    if (!invokesMap.has(cmd)) {
      invokesMap.set(cmd, new Set());
    }
    invokesMap.get(cmd).add(file.replace(/\\/g, '/'));
  }
}

const libRs = fs.readFileSync('apps/desktop/src-tauri/src/lib.rs', 'utf8');
const handlerMatch = libRs.match(/generate_handler!\[([\s\S]*?)\]/);
if (!handlerMatch) {
  console.error('generate_handler not found');
  process.exit(1);
}
const registeredCmds = new Set(
  handlerMatch[1]
    .split(',')
    .map(s => s.trim())
    .filter(s => s && !s.startsWith('//'))
);

console.log('Total unique invokes found in frontend:', invokesMap.size);
console.log('Total commands registered in Tauri generate_handler:', registeredCmds.size);

const missing = [];
const found = [];

for (const [cmd, files] of invokesMap.entries()) {
  if (registeredCmds.has(cmd)) {
    found.push(cmd);
  } else {
    missing.push({ cmd, files: Array.from(files) });
  }
}

console.log('\n=== MISSING COMMANDS (' + missing.length + ') ===');
for (const m of missing) {
  console.log('- ' + m.cmd + ' (used in ' + m.files.join(', ') + ')');
}
