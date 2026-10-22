const fs = require('fs');
const path = require('path');

const en = JSON.parse(fs.readFileSync('src/i18n/locales/en.json', 'utf8'));
const vi = JSON.parse(fs.readFileSync('src/i18n/locales/vi.json', 'utf8'));

function getNested(obj, pathStr) {
  const parts = pathStr.split('.');
  let curr = obj;
  for (const p of parts) {
    if (curr === undefined || curr === null) return undefined;
    curr = curr[p];
  }
  return curr;
}

function getAllFiles(dir, exts) {
  let res = [];
  for (const item of fs.readdirSync(dir)) {
    const full = path.join(dir, item);
    const stat = fs.statSync(full);
    if (stat.isDirectory()) {
      if (item !== 'node_modules' && item !== 'dist' && item !== '.git') {
        res = res.concat(getAllFiles(full, exts));
      }
    } else if (exts.some(ext => item.endsWith(ext))) {
      res.push(full);
    }
  }
  return res;
}

const files = getAllFiles('src', ['.ts', '.tsx']);
const keyRegex = /\bt\(\s*["']([a-zA-Z0-9_\.\-]+)["']/g;

const uiMissingEn = new Map();
const backendMissingEn = new Map();

for (const f of files) {
  const isBackend = f.includes('backend-errors.ts');
  const content = fs.readFileSync(f, 'utf8');
  let match;
  while ((match = keyRegex.exec(content)) !== null) {
    const key = match[1];
    if (key.includes('{{') || key.startsWith('http') || key.length < 3) continue;
    if (getNested(en, key) === undefined) {
      const targetMap = isBackend ? backendMissingEn : uiMissingEn;
      if (!targetMap.has(key)) targetMap.set(key, []);
      targetMap.get(key).push(path.basename(f));
    }
  }
}

fs.writeFileSync('scripts/missing_ui.json', JSON.stringify(Object.fromEntries(uiMissingEn), null, 2));
fs.writeFileSync('scripts/missing_backend.json', JSON.stringify(Object.fromEntries(backendMissingEn), null, 2));
console.log('UI Missing count:', uiMissingEn.size);
console.log('Backend Missing count:', backendMissingEn.size);
