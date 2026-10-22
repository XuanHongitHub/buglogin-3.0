const fs = require('fs');

const currentEn = JSON.parse(fs.readFileSync('src/i18n/locales/en.json', 'utf8'));
const currentVi = JSON.parse(fs.readFileSync('src/i18n/locales/vi.json', 'utf8'));

const upstreamEn = JSON.parse(fs.readFileSync('.tmp/donutbrowser-upstream/src/i18n/locales/en.json', 'utf8'));
const upstreamVi = JSON.parse(fs.readFileSync('.tmp/donutbrowser-upstream/src/i18n/locales/vi.json', 'utf8'));

// Deep merge: objSource into objTarget without overwriting existing non-null keys in objTarget
function deepMergeIfMissing(target, source) {
  for (const key of Object.keys(source)) {
    if (source[key] && typeof source[key] === 'object' && !Array.isArray(source[key])) {
      if (!target[key] || typeof target[key] !== 'object') {
        target[key] = {};
      }
      deepMergeIfMissing(target[key], source[key]);
    } else {
      if (target[key] === undefined) {
        target[key] = source[key];
      }
    }
  }
}

// 1. Merge all upstream keys into current
deepMergeIfMissing(currentEn, upstreamEn);
deepMergeIfMissing(currentVi, upstreamVi);

// Helper to set nested key
function setNested(obj, pathStr, value) {
  const parts = pathStr.split('.');
  let curr = obj;
  for (let i = 0; i < parts.length - 1; i++) {
    const p = parts[i];
    if (!curr[p] || typeof curr[p] !== 'object') {
      curr[p] = {};
    }
    curr = curr[p];
  }
  curr[parts[parts.length - 1]] = value;
}

// 2. Custom keys
const customKeysEn = {
  'agent.setup.remoteUnavailableTitle': 'Remote Agent Unavailable',
  'sync.cloud.serviceUnavailable': 'Cloud sync service is currently unavailable. Please try again later.',
  'cookieBot.history.sitesFailed': 'Failed sites',
  'cookieBot.history.consentHandled': 'Consent handled',
  'cookieBot.live.consentHandled': 'Consent handled',
  'cookieBot.enrol.enrolled': 'Enrolled',
  'cookieBot.enrol.titleCount': 'Enrol Profiles ({{count}})',
  'cookieBot.preflight.ineligible': 'Ineligible',
  'cookieBot.enrol.sitesHint': 'Enter site URLs, one per line',
  'cookieBot.enrol.listSites': 'Target Sites',
  'cookieBot.lastDay.ran': 'Ran',
  'cookieBot.lastDay.partial': 'Partial',
  'cookieBot.lastDay.failed': 'Failed',
  'cookieBot.tonight.dueCount': '{{count}} due tonight',
  'cookieBot.picker.continue': 'Continue',
  'cookieBot.enrol.cadenceCustom': 'Custom cadence',
  'dnsBlocklist.custom.saved': 'DNS blocklist rules saved',
  'dnsBlocklist.custom.imported': 'Rules imported successfully',
  'dnsBlocklist.custom.exported': 'Rules exported successfully',
  'dnsBlocklist.custom.description': 'Configure custom DNS domain blocking and allowlists.',
  'dnsBlocklist.custom.allowlistModeLabel': 'Allowlist Mode',
  'dnsBlocklist.custom.allowlistModeOn': 'Enabled (only allowed domains resolve)',
  'dnsBlocklist.custom.allowlistModeOff': 'Disabled (block listed domains)',
  'dnsBlocklist.custom.sourcesLabel': 'Blocklist URL Sources',
  'dnsBlocklist.custom.sourcesPlaceholder': 'https://example.com/blocklist.txt',
  'dnsBlocklist.custom.blockLabel': 'Blocked Domains',
  'dnsBlocklist.custom.blockPlaceholder': 'example.com',
  'dnsBlocklist.custom.allowedOnlyLabel': 'Strict Allowlist Only',
  'dnsBlocklist.custom.allowLabel': 'Allowed Domains',
  'dnsBlocklist.custom.allowPlaceholder': 'allowed.com',
  'dnsBlocklist.custom.allowedOnlyHint': 'When enabled, any domain not explicitly listed below will be blocked.',
  'dnsBlocklist.custom.allowHint': 'Domains that bypass blocklists.',
  'dnsBlocklist.custom.exportTxt': 'Export as TXT',
  'dnsBlocklist.custom.exportJson': 'Export as JSON',
  'profilePassword.errors.passwordMismatch': 'Passwords do not match',
  'cookieBot.team.pooled': 'Pooled',
  'trash.emptied': 'Trash emptied successfully',
  'trash.expiresIn': 'Expires in {{days}} days',
  'trash.confirmEmptyDescription': 'Are you sure you want to permanently delete all items in trash? This action cannot be undone.',
  'header.newWorkspace': 'New Workspace',
  'backendErrors.groupNotEmpty': 'Cannot delete group: it still contains profiles or extensions.'
};

const customKeysVi = {
  'agent.setup.remoteUnavailableTitle': 'Tác nhân từ xa không khả dụng',
  'sync.cloud.serviceUnavailable': 'Dịch vụ đồng bộ đám mây hiện không khả dụng. Vui lòng thử lại sau.',
  'cookieBot.history.sitesFailed': 'Trang thất bại',
  'cookieBot.history.consentHandled': 'Đã xử lý đồng ý cookie',
  'cookieBot.live.consentHandled': 'Đã xử lý đồng ý cookie',
  'cookieBot.enrol.enrolled': 'Đã đăng ký',
  'cookieBot.enrol.titleCount': 'Đăng ký hồ sơ ({{count}})',
  'cookieBot.preflight.ineligible': 'Không đủ điều kiện',
  'cookieBot.enrol.sitesHint': 'Nhập URL trang web, mỗi trang một dòng',
  'cookieBot.enrol.listSites': 'Danh sách trang mục tiêu',
  'cookieBot.lastDay.ran': 'Đã chạy',
  'cookieBot.lastDay.partial': 'Một phần',
  'cookieBot.lastDay.failed': 'Thất bại',
  'cookieBot.tonight.dueCount': '{{count}} cần chạy tối nay',
  'cookieBot.picker.continue': 'Tiếp tục',
  'cookieBot.enrol.cadenceCustom': 'Chu kỳ tùy chỉnh',
  'dnsBlocklist.custom.saved': 'Đã lưu quy tắc chặn DNS',
  'dnsBlocklist.custom.imported': 'Đã nhập quy tắc thành công',
  'dnsBlocklist.custom.exported': 'Đã xuất quy tắc thành công',
  'dnsBlocklist.custom.description': 'Cấu hình danh sách chặn và danh sách cho phép tên miền DNS tùy chỉnh.',
  'dnsBlocklist.custom.allowlistModeLabel': 'Chế độ danh sách cho phép',
  'dnsBlocklist.custom.allowlistModeOn': 'Đang bật (chỉ phân giải các tên miền được phép)',
  'dnsBlocklist.custom.allowlistModeOff': 'Đang tắt (chặn các tên miền trong danh sách)',
  'dnsBlocklist.custom.sourcesLabel': 'Nguồn danh sách chặn URL',
  'dnsBlocklist.custom.sourcesPlaceholder': 'https://example.com/blocklist.txt',
  'dnsBlocklist.custom.blockLabel': 'Tên miền bị chặn',
  'dnsBlocklist.custom.blockPlaceholder': 'example.com',
  'dnsBlocklist.custom.allowedOnlyLabel': 'Chỉ cho phép nghiêm ngặt',
  'dnsBlocklist.custom.allowLabel': 'Tên miền được phép',
  'dnsBlocklist.custom.allowPlaceholder': 'allowed.com',
  'dnsBlocklist.custom.allowedOnlyHint': 'Khi bật, mọi tên miền không có trong danh sách dưới đây sẽ bị chặn.',
  'dnsBlocklist.custom.allowHint': 'Các tên miền bỏ qua danh sách chặn.',
  'dnsBlocklist.custom.exportTxt': 'Xuất file TXT',
  'dnsBlocklist.custom.exportJson': 'Xuất file JSON',
  'profilePassword.errors.passwordMismatch': 'Mật khẩu không khớp',
  'cookieBot.team.pooled': 'Dùng chung',
  'trash.emptied': 'Đã dọn sạch thùng rác',
  'trash.expiresIn': 'Hết hạn sau {{days}} ngày',
  'trash.confirmEmptyDescription': 'Bạn có chắc chắn muốn xóa vĩnh viễn tất cả mục trong thùng rác không? Hành động này không thể hoàn tác.',
  'header.newWorkspace': 'Không gian làm việc mới',
  'backendErrors.groupNotEmpty': 'Không thể xóa nhóm: nhóm vẫn còn chứa hồ sơ hoặc tiện ích mở rộng.'
};

for (const [k, v] of Object.entries(customKeysEn)) {
  setNested(currentEn, k, v);
}

for (const [k, v] of Object.entries(customKeysVi)) {
  setNested(currentVi, k, v);
}

// 3. Write back with 2 space indentation and UTF-8
fs.writeFileSync('src/i18n/locales/en.json', JSON.stringify(currentEn, null, 2) + '\n', 'utf8');
fs.writeFileSync('src/i18n/locales/vi.json', JSON.stringify(currentVi, null, 2) + '\n', 'utf8');

console.log('Merge complete!');
