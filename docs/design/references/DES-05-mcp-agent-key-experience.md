# DES-05 Reference Package: Dedicated MCP API Key & AI Agent Quick-Connect Experience

Status: approved design specification & interaction contract  
Screen: Integrations Dialog (`/integrations` -> Tab `Trợ lý AI (MCP Protocol)`)  
Authority: [`docs/design/ui-authority.md`](../ui-authority.md), [`DESIGN.md`](../../DESIGN.md), [`DES-02-unified-switcher-action-hub.md`](DES-02-unified-switcher-action-hub.md)  
Last updated: 2026-10-26  

---

## 1. Mục Tiêu và Triệt Tiêu Vấn Đề (Problem Statement & Goals)

### 1.1. Thực trạng trước khi tối ưu (Current Flaws)
1. **Tràn lan key rác trùng lặp (Orphan Key Proliferation):** Mỗi lần người dùng vào tab "Trợ lý AI (MCP Protocol)" và nhấn "Tạo key MCP", hệ thống lại sinh ra một token mới và dồn ứ danh sách ngày càng dài (`BugLogin MCP · hết hạn 1/21/2027`, `BugLogin MCP · hết hạn 1/22/2027`...). Người dùng không biết đâu là key đang được AI Agent sử dụng.
2. **Mất chuỗi bí mật ngay sau khi đóng hộp thoại (Lost Secret Blindness):** Backend chỉ lưu chuỗi băm `sha256(token)`. Khi người dùng đóng dialog hoặc chuyển tab, token trong bộ nhớ RAM bị mất hoàn toàn. Người dùng không thể xem lại mã bí mật, không thể sao chép, buộc phải tạo thêm key mới mỗi khi cấu hình lại Cursor/Claude.
3. **Thiếu nút Hiện / Ẩn bảo mật (No Mask / Show-Hide Toggle):** Giao diện không có cơ chế ẩn chuỗi secret để tránh bị nhìn trộm khi quay màn hình hoặc stream trực tiếp, đồng thời không có nút bật để đối chiếu khi cần kiểm tra.
4. **Không có nút sao chép API Key riêng biệt (Missing Direct Copy Key Action):** Danh sách key cũ chỉ có nút "Thu hồi", hoàn toàn không có nút "Sao chép key" thô.
5. **Cấu hình 1-Click JSON bị khóa chết (Disabled Config JSON Snippet):** Khối JSON mẫu hiển thị `YOUR_BUGLOGIN_API_KEY` và nút "Sao chép JSON" bị disable (`disabled={!newMcpToken}`), khiến trải nghiệm 1-Click Setup hoàn toàn vô dụng khi mở lại tab.

### 1.2. Mục tiêu thiết kế mới (Target Goals)
1. **1 Key Cố Định Duy Nhất (Single Dedicated MCP Key):** Duy trì duy nhất 1 Key MCP cố định cho tài khoản trên thiết bị. Không cho phép tạo tràn lan key rác. Key có thời hạn 1 năm (`365 ngày`) để đảm bảo tính ổn định dài hạn cho các AI Agent.
2. **Lưu trữ an toàn & bền bỉ (Local Vault Persistence):** Key được lưu trữ mã hóa cục bộ trên máy khách (`localStorage` / desktop vault), đảm bảo luôn sẵn sàng phục vụ mà không cần tạo lại.
3. **Hiển thị linh hoạt với nút Hiện / Ẩn (Masked by Default + Show/Hide):** Mặc định hiển thị dạng ẩn `blapi_••••••••••••••••••••••••••••••••`, kèm nút con mắt (`LuEye` / `LuEyeOff`) để bật/tắt hiển thị chuỗi thật.
4. **Sao chép tức thì 1 chạm (Instant 1-Click Copy Key):** Nút "Sao chép key" (`LuCopy` / `LuCheck`) luôn sẵn sàng, copy chuỗi token thô vào clipboard kèm phản hồi thị giác rõ nét.
5. **Tự động điền (Auto-Injection) vào 1-Click JSON:** Cấu hình JSON cho Cursor, Claude Desktop, Antigravity/Windsurf luôn được tự động chèn sẵn chuỗi Key cố định, người dùng chỉ cần nhấn "Sao chép JSON" và dán là chạy ngay.
6. **Cơ chế Tự dọn dẹp & Kích hoạt 1 chạm (Self-Healing Orphan Cleanup):** Nếu tài khoản đang có các key cũ tồn đọng từ các phiên trước, giao diện cung cấp nút "Kích hoạt Key cố định duy nhất" để tự động thu hồi toàn bộ key cũ và cấp 1 key chuẩn mới.

---

## 2. Chu Trình Vòng Đời Key MCP (Lifecycle State Machine)

```text
[TRẠNG THÁI 1: CHƯA CÓ KEY]
       │
       ▼ (Bấm "Tạo Key MCP Cố Định")
[TRẠNG THÁI 2: ĐANG TẠO / SINH TOKEN 365 NGÀY]
       │
       ▼ (Tạo thành công + Lưu Local Vault + Cập nhật UI)
[TRẠNG THÁI 3: KEY CỐ ĐỊNH HOẠT ĐỘNG (ACTIVE)]
       ├── Mặc định: Ẩn chuỗi (blapi_••••••••••••••••)
       ├── Bấm [👁️ Hiện]: Chuyển sang chuỗi đầy đủ (blapi_1a2b3c4d...)
       ├── Bấm [📋 Sao chép]: Copy chuỗi thô vào clipboard
       ├── JSON 1-Click: Tự động điền "Authorization: Bearer blapi_..."
       │
       ├──► Bấm [🔄 Tạo lại key]: Thu hồi key cũ ──► Cấp key mới ──► Lưu đè Local Vault
       │
       └──► Bấm [🗑️ Thu hồi]: Thu hồi trên server ──► Xóa Local Vault ──► Quay về Trạng thái 1

[TRẠNG THÁI ĐẶC BIỆT: PHÁT HIỆN KEY CŨ RÁC TỒN ĐỌNG (MÃ BÍ MẬT CHƯA LƯU CỤC BỘ)]
       │
       ▼ (Bấm "Kích hoạt Key cố định duy nhất")
Thu hồi tự động tất cả key cũ ──► Tạo 1 key cố định mới ──► Lưu Local Vault ──► Sang Trạng thái 3
```

---

## 3. Sơ Đồ Bố Cục ASCII (Master Layouts)

### 3.1. Trạng Thái Đang Hoạt Động (Active Dedicated Key - Mặc Định Ẩn Masked)

```text
+---------------------------------------------------------------------------------------------------+
| [LuBot] Model Context Protocol (MCP)                                   [🟢 MCP sẵn sàng kết nối]  |
| Chuẩn giao thức mở của Anthropic cho phép các trợ lý AI (Cursor, Claude Desktop, Antigravity)     |
| trực tiếp mở browser, tự động hóa tác vụ trên BugLogin.                                           |
+---------------------------------------------------------------------------------------------------+
|  [LuKey] API KEY CỐ ĐỊNH CHO MCP                                              [🟢 Đang hoạt động] |
|  Key cố định dùng kết nối các trợ lý AI. Được lưu an toàn trên máy này.                           |
|                                                                                                   |
|  +------------------------------------------------------------+ [👁️ Hiện]  [📋 Sao chép key]     |
|  | blapi_•••••••••••••••••••••••••••••••••••••••••••••••••••  |                                   |
|  +------------------------------------------------------------+ [🔄 Tạo lại] [🗑️ Thu hồi]       |
|                                                                                                   |
|  ℹ️ Hết hạn: 26/10/2027 (Thời hạn 1 năm) · Quyền: profile, sync  [🟢 Đã tự động điền cấu hình]    |
+---------------------------------------------------------------------------------------------------+
| MCP SERVER ENDPOINT                                                                               |
| https://api3.buglogin.com/v1/mcp                                                [📋 Sao chép URL] |
+---------------------------------------------------------------------------------------------------+
| [LuCpu] Cấu hình AI Agent (1-Click Setup)                     [Cursor AI] [Claude] [Antigravity]  |
| Dán đoạn mã cấu hình dưới đây vào file cài đặt MCP của ứng dụng AI bạn đang dùng.                 |
|                                                                                                   |
| Đường dẫn tệp: .cursor/mcp.json hoặc Cursor Settings -> Features -> MCP                          |
| +-----------------------------------------------------------------------------------------------+ |
| |                                                                           [📋 Sao chép JSON]  | |
| | {                                                                                             | |
| |   "mcpServers": {                                                                             | |
| |     "buglogin": {                                                                             | |
| |       "url": "https://api3.buglogin.com/v1/mcp",                                              | |
| |       "headers": {                                                                            | |
| |         "Authorization": "Bearer blapi_0192837465abcdef0192837465abcdef"                     | |
| |       }                                                                                       | |
| |     }                                                                                         | |
| |   }                                                                                           | |
| | }                                                                                             | |
| +-----------------------------------------------------------------------------------------------+ |
| 🟢 Đã tự động điền Key MCP cố định của bạn vào file cấu hình. Chỉ cần sao chép và dán.            |
+---------------------------------------------------------------------------------------------------+
```

### 3.2. Khi Người Dùng Bấm Nút [👁️ Hiện] (Unmasked State)

```text
|  +------------------------------------------------------------+ [👁️‍🗨️ Ẩn]   [📋 Sao chép key]     |
|  | blapi_0192837465abcdef0192837465abcdef9876543210fedcba     |                                   |
|  +------------------------------------------------------------+ [🔄 Tạo lại] [🗑️ Thu hồi]       |
```

### 3.3. Khi Người Dùng Bấm Nút [📋 Sao chép key] (Copied Feedback)

```text
|  +------------------------------------------------------------+ [👁️ Hiện]  [✅ Đã chép key]     |
|  | blapi_•••••••••••••••••••••••••••••••••••••••••••••••••••  |                                   |
|  +------------------------------------------------------------+ [🔄 Tạo lại] [🗑️ Thu hồi]       |
```

### 3.4. Trạng Thái Tự Dọn Dẹp Key Cũ (Self-Healing Orphan Cleanup State)

```text
+---------------------------------------------------------------------------------------------------+
| [LuKey] API KEY CỐ ĐỊNH CHO MCP                                              [🟡 Cần đồng bộ key] |
|                                                                                                   |
| +-----------------------------------------------------------------------------------------------+ |
| | [LuTriangleAlert] Phát hiện 2 key MCP cũ trên tài khoản (Chưa lưu mã bí mật trên máy này)     | |
| | Chuỗi bí mật chưa được lưu cố định trên thiết bị này nên không thể Hiện hay Sao chép trực tiếp| |
| | Hãy bấm nút bên dưới để dọn dẹp các key cũ và kích hoạt 1 Key cố định duy nhất.               | |
| |                                                                                               | |
| | [ ⚡ Kích hoạt Key MCP Cố Định Duy Nhất ]                 [ Thu hồi các key cũ ]              | |
| +-----------------------------------------------------------------------------------------------+ |
+---------------------------------------------------------------------------------------------------+
```

---

## 4. Đặc Tả Tương Tác & Nguyên Tắc Bảo Mật (Security & Interaction Rules)

1. **Không rò rỉ khi không chủ ý (Default Masking):** Khi mở màn hình, key luôn ở trạng thái `masked` để người dùng an tâm khi chia sẻ màn hình.
2. **Clipboard an toàn:** Khi bấm "Sao chép key", ứng dụng lấy trực tiếp chuỗi token thô trong bộ nhớ an toàn đưa vào clipboard, không phụ thuộc vào trạng thái ẩn/hiện của giao diện.
3. **Auto-injection chuẩn xác:** Mẫu JSON cho Cursor AI, Claude Desktop và Antigravity/Windsurf luôn chứa đúng chuỗi `Bearer blapi_...`. Nút "Sao chép JSON" không bao giờ bị khóa vô cớ khi đã có key.
4. **Dọn sạch rác khi tạo mới:** Mọi hành động Tạo mới hoặc Tạo lại đều tự động thu hồi các token MCP cũ của tài khoản, triệt tiêu 100% tình trạng nhiều dòng key rác như phiên bản trước.
