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

const libRs = fs.readFileSync('apps/desktop/src-tauri/src/lib.rs', 'utf8');
const handlerMatch = libRs.match(/generate_handler!\[([\s\S]*?)\]\)/);
const cleaned = handlerMatch ? handlerMatch[1].replace(/\/\/.*$/gm, '') : '';
const registeredCmds = new Set(
  cleaned
    .split(',')
    .map(s => s.trim())
    .filter(Boolean)
    .map(s => s.split('::').pop().trim())
);

const fileToInvokes = new Map();

for (const file of frontendFiles) {
  const content = fs.readFileSync(file, 'utf8');
  let match;
  const missingInFile = new Set();
  const re = /invoke(?:<[^>]*>)?\(\s*["']([a-zA-Z0-9_]+)["']/g;
  while ((match = re.exec(content)) !== null) {
    const cmd = match[1];
    if (!registeredCmds.has(cmd)) {
      missingInFile.add(cmd);
    }
  }
  if (missingInFile.size > 0) {
    fileToInvokes.set(file.replace(/\\/g, '/'), Array.from(missingInFile));
  }
}

console.log('Files with missing invokes:', fileToInvokes.size);
for (const [file, cmds] of fileToInvokes.entries()) {
  console.log('\n[' + file + '] (' + cmds.length + '):');
  console.log('  ' + cmds.join(', '));
}
