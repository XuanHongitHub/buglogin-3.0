# BugLogin — Báo Cáo & Đặc Tả Thiết Kế Chi Tiết UI/UX Master

Tài liệu này đặc tả chi tiết giao diện và trải nghiệm người dùng (UI/UX) cho 5 phân hệ trọng tâm của **BugLogin**:
1. **Trang Đăng Nhập (Login Page)**
2. **Quản Lý Tài Khoản (Account Page)**
3. **Quản Lý Tổ Chức & Không Gian Làm Việc (Org & Workspace Management)**
4. **Phân Quyền & Thành Viên (Member RBAC & Permissions)**
5. **Quản Lý Nhóm & Profile (Group Management & Profile Hierarchy)**

---

## 1. TRANG ĐĂNG NHẬP (LOGIN PAGE)

### 1.1. Hiện Tại (Current Implementation — `AuthBootstrap.tsx`)
```text
+--------------------------------------------------------------------------------------------------------+
|                                                                                                        |
|   +-------------------------------------------------+----------------------------------------------+   |
|   |  [Logo 40x40]                [VI|EN]  [☼/☽ Theme]|                                              |   |
|   |                                                 |                                              |   |
|   |               Đăng nhập để tiếp tục              |                                              |   |
|   |                                                 |                                              |   |
|   |            +-------------------------+          |                                              |   |
|   |            |   ✉ Đăng nhập           |          |         [HERO BANNER MINIMAL IMAGE]          |   |
|   |            +-------------------------+          |                                              |   |
|   |                                                 |   Ảnh minh họa nền tảng BugLogin             |   |
|   |   Email:                                        |   Multi-Account & Antidetect                 |   |
|   |   +-----------------------------------------+   |                                              |   |
|   |   | admin@example.com                       |   |   /img_buglogin/                             |   |
|   |   +-----------------------------------------+   |   browser-multi-account-platform.webp        |   |
|   |                                                 |                                              |   |
|   |   Mật khẩu:                                     |                                              |   |
|   |   +-------------------------------------+---+   |                                              |   |
|   |   | ••••••••••••••••••••••••••••••••••••| 👁 |   |                                              |   |
|   |   +-------------------------------------+---+   |                                              |   |
|   |                                                 |                                              |   |
|   |   [x] Ghi nhớ đăng nhập                         |                                              |   |
|   |                                                 |                                              |   |
|   |   +-----------------------------------------+   |                                              |   |
|   |   |        Đăng nhập bằng email             |   |                                              |   |
|   |   +-----------------------------------------+   |                                              |   |
|   |                                                 |                                              |   |
|   +-------------------------------------------------+----------------------------------------------+   |
|                                                                                                        |
+--------------------------------------------------------------------------------------------------------+
```

### 1.2. Nhận Xét Hiện Tại & Điểm Cần Cải Thiện
- ✅ **Ưu điểm**: Đã gỡ bỏ chia đôi tab Local/Cloud, form tập trung, có toggle xem password và song ngữ Anh-Việt.
- ⚠️ **Khuyết điểm**:
  - Không có chỉ báo trạng thái kết nối backend local (`Native Core Service`), nếu backend crash thì user bấm đăng nhập chỉ báo lỗi generic.
  - Thiếu cảnh báo Caps Lock khi gõ mật khẩu.
  - Thiếu link hỗ trợ / tài liệu hướng dẫn khi quên mật khẩu hoặc tài khoản chưa kích hoạt.
  - Nút đăng nhập chưa có icon đồng bộ hệ thống.

---

### 1.3. Best UI/UX (Đề Xuất Chuẩn Hóa Chi Tiết)
```text
+--------------------------------------------------------------------------------------------------------+
|                                                                                                        |
|   +-------------------------------------------------+----------------------------------------------+   |
|   |  [Logo BugLogin]              [VI | EN]  [☼/☽]  |                                              |   |
|   |                                                 |                                              |   |
|   |   BUGLOGIN                                      |                                              |   |
|   |   Nền tảng Quản lý Trình duyệt & Antidetect     |         [HERO BANNER MINIMAL IMAGE]          |   |
|   |   ───────────────────────────────────────────   |                                              |   |
|   |                                                 |   Ảnh minh họa sản phẩm sắc nét              |   |
|   |   Email hoặc Tài khoản                          |   Tỷ lệ 1:1, căn giữa dọc                    |   |
|   |   +-----------------------------------------+   |                                              |   |
|   |   | ✉  name@company.com                     |   |   Tối ưu hiển thị cho cả                     |   |
|   |   +-----------------------------------------+   |   Dark Mode & Light Mode                     |   |
|   |                                                 |                                              |   |
|   |   Mật khẩu                 [Quên mật khẩu?]     |                                              |   |
|   |   +-------------------------------------+---+   |                                              |   |
|   |   | 🔒 •••••••••••••••••••••••••••••••••| 👁 |   |                                              |   |
|   |   +-------------------------------------+---+   |                                              |   |
|   |   [!] Caps Lock đang bật (chỉ hiện khi CapsLock)|                                              |   |
|   |                                                 |                                              |   |
|   |   [x] Ghi nhớ tài khoản trên thiết bị này       |                                              |   |
|   |                                                 |                                              |   |
|   |   +-----------------------------------------+   |                                              |   |
|   |   |    ➜]  Đăng Nhập                        |   |                                              |   |
|   |   +-----------------------------------------+   |                                              |   |
|   |                                                 |                                              |   |
|   |   ───────────────────────────────────────────   |                                              |   |
|   |   ● Trạng thái kết nối: Đám mây hoạt động (Cloud Sync Online)                                  |   |
|   +-------------------------------------------------+----------------------------------------------+   |
|                                                                                                        |
+--------------------------------------------------------------------------------------------------------+
```

---

## 2. QUẢN LÝ TÀI KHOẢN (ACCOUNT PAGE)

### 2.1. Hiện Tại (Current Implementation — `account-page.tsx`)
```text
+------------------------------------------------------------------------------+
| [👤] Thông tin Tài khoản                                 [👑 PRO ENTERPRISE] |
|      Quản lý gói dịch vụ, hạn mức và bảo mật                                 |
| ──────────────────────────────────────────────────────────────────────────── |
|                                                                              |
|  +------------------------------------------------------------------------+  |
|  | [U]  user@example.com                                      [🔄 Làm mới]|  |
|  |      🛡️ Owner (Chủ sở hữu)  •  Hạn dùng: 2027-12-31                     |  |
|  +------------------------------------------------------------------------+  |
|                                                                              |
|  Tài nguyên & Hạn mức sử dụng                                                |
|  +---------------------+  +---------------------+  +---------------------+   |
|  | 💻 Profiles     62% |  | 👥 Thành viên   24% |  | 🏢 Workspaces Active|   |
|  | [████████░░░░░░░░░] |  | [███░░░░░░░░░░░░░░░]|  | [████░░░░░░░░░░░░░░]|   |
|  | 620 / 1,000 slots   |  | 12 / 50 seats       |  | 3 Workspaces        |   |
|  +---------------------+  +---------------------+  +---------------------+   |
|                                                                              |
|  +------------------------------------------------------------------------+  |
|  | 💽 Thiết bị hiện tại: Windows 11 (Native Desktop)             ● Active |  |
|  +------------------------------------------------------------------------+  |
|                                                                              |
| ──────────────────────────────────────────────────────────────────────────── |
| [🚪 Đăng xuất]                                                        [Đóng] |
+------------------------------------------------------------------------------+
```

### 2.2. Nhận Xét Hiện Tại & Điểm Cần Cải Thiện
- ⚠️ Chưa có tính năng **Đổi mật khẩu** (Change Password).
- ⚠️ Hạn mức tài nguyên chỉ là thẻ cứng, chưa hiển thị chi tiết (ví dụ: số profile đám mây vs số profile cục bộ).
- ⚠️ Chưa có danh sách các máy/thiết bị khác đang cùng đăng nhập (Active Sessions) để bấm thu hồi phiên từ xa khi bị lộ tài khoản.
- ⚠️ Chưa có nút sao chép User ID hoặc License Key để gửi support khi cần kích hoạt.

---

### 2.3. Best UI/UX (Đề Xuất Chuẩn Hóa Chi Tiết)
```text
+-----------------------------------------------------------------------------------------+
| [👤] Tài Khoản & Gói Dịch Vụ                                        [👑 PRO ENTERPRISE] |
|      Quản lý thông tin định danh, hạn mức và phiên đăng nhập bảo mật                     |
| ─────────────────────────────────────────────────────────────────────────────────────── |
|                                                                                         |
|  [ Thẻ Tổng Quan & Hồ Sơ ]                                                              |
|  +-----------------------------------------------------------------------------------+  |
|  | [Avatar]  Nguyen Van A (Owner)                             [ 📋 Copy ID ] [🔄 F5] |  |
|  |           Email: founder@buglogin.com  •  ID: acc_8f93a201b                        |  |
|  |           Trạng thái: ● Đã xác thực  •  Thời hạn gói: Còn 428 ngày (đến 2027-12-31)|  |
|  +-----------------------------------------------------------------------------------+  |
|                                                                                         |
|  Tabs: [ 📊 Tài Nguyên & Hạn Mức ]   [ 🔐 Bảo Mật & Mật Khẩu ]   [ 💻 Thiết Bị (2) ]     |
| ─────────────────────────────────────────────────────────────────────────────────────── |
|                                                                                         |
|  (NỘI DUNG TAB 1: TÀI NGUYÊN & HẠN MỨC GÓI)                                             |
|  +------------------------+  +------------------------+  +------------------------+     |
|  | 💻 Profiles Slot   62% |  | 👥 Ghế Thành Viên  24% |  | 🏢 Workspaces       3/5|     |
|  | [████████████░░░░░░░░] |  | [████░░░░░░░░░░░░░░░░] |  | [██████░░░░░░░░░░░░░░] |     |
|  | 620 / 1,000 Profiles   |  | 12 / 50 Thành viên     |  | 3 Active Workspaces    |     |
|  | • Đồng bộ Cloud: 100%  |  | • Quản trị viên: 3     |  | • 1 Workspace Cá nhân  |     |
|  | • Khóa E2EE: Bảo vệ    |  | • Nhân viên (Op): 9    |  | • 2 Workspace Nhóm/Org |     |
|  +------------------------+  +------------------------+  +------------------------+     |
|                                                                                         |
|  (NỘI DUNG TAB 2: BẢO MẬT & ĐỔI MẬT KHẨU)                                               |
|  Mật khẩu hiện tại:    [ ••••••••••••••••                               ]               |
|  Mật khẩu mới:         [ ••••••••••••••••                               ]               |
|  Nhập lại MK mới:      [ ••••••••••••••••                               ]               |
|  [ Lưu Mật Khẩu Mới ]                                                                   |
|                                                                                         |
|  (NỘI DUNG TAB 3: THIẾT BỊ ĐANG ĐĂNG NHẬP)                                              |
|  • 🖥️ Desktop (Thiết bị này) - Windows 11 (IP: 14.232.x.x)       ● Đang chạy           |
|  • 💻 MacBook Pro M2 - macOS 14.5 (IP: 118.69.x.x)              [ ✕ Thu hồi phiên ]     |
|                                                                                         |
| ─────────────────────────────────────────────────────────────────────────────────────── |
| [🚪 Đăng Xuất Tài Khoản]                                      [ Nâng Cấp Gói ]  [ Đóng ]|
+-----------------------------------------------------------------------------------------+
```

---

## 3. QUẢN LÝ TỔ CHỨC (ORG) & WORKSPACE

### 3.1. Hiện Tại (Current Implementation — `workspace-switcher.tsx`)
```text
+------------------------------+
| [🏢 Workspace A          ▼ ] |  <-- Nút popover góc trên trái
+------------------------------+
| [ 🔍 Find workspace...     ] |
|                              |
| PERSONAL                     |
|   👤 Personal Workspace    ✓ |
|                              |
| ORGANIZATIONS                |
|   🏢 Agency Marketing        |
|   🏢 TikTok Shop Team        |
|                              |
| ──────────────────────────── |
| [ + Create Workspace       ] |
+------------------------------+
```

### 3.2. Nhận Xét Hiện Tại & Điểm Cần Cải Thiện
- ⚠️ Chỉ là 1 dropdown chọn nhanh, **hoàn toàn chưa có màn hình quản trị chi tiết Workspace & Tổ chức**.
- ⚠️ Không xem được workspace này do ai tạo, có bao nhiêu profile, bao nhiêu thành viên.
- ⚠️ Không đổi tên được workspace, không phân loại được theo Organization.
- ⚠️ Không cấu hình được quyền hạn hoặc xóa workspace không dùng.

---

### 3.3. Best UI/UX: Màn Hình Quản Trị Tổ Chức & Workspace (`WorkspaceManagementDialog`)
```text
+--------------------------------------------------------------------------------------------------------+
| [🏢] Quản Lý Tổ Chức & Không Gian Làm Việc (Workspaces)                                   [ ✕ Đóng ]   |
|      Phân chia môi trường vận hành riêng biệt giữa các dự án, khách hàng và phòng ban                   |
| ────────────────────────────────────────────────────────────────────────────────────────────────────── |
|                                                                                                        |
|  Tổ Chức Hiện Tại: [ 🏢 BugLogin Media Agency  ▼ ]               [ + Tạo Workspace Mới ] [ ⚙ Cài Đặt Org]|
|                                                                                                        |
|  Tìm kiếm: [ 🔍 Tìm tên workspace...                         ]   Lọc theo: [ Tất cả (3) ▼ ]            |
|                                                                                                        |
|  +--------------------------------------------------------------------------------------------------+  |
|  | Tên Workspace          | Tổ Chức       | Profiles | Thành Viên | Vai Trò Của Bạn | Trạng Thái| Thao Tác|
|  |------------------------+---------------+----------+------------+-----------------+-----------+---------|
|  | ⭐ Default Personal    | Cá nhân       | 142      | 1 (Chỉ bạn)| Owner           | ● Đang chọn| [⚙ Sửa] |
|  | 🏢 Shopee Ads Global   | Media Agency  | 320      | 8 members  | Admin           | [Chuyển]  | [⚙][👥] |
|  | 🏢 TikTok Shop Affiliate| Media Agency | 158      | 4 members  | Operator        | [Chuyển]  | [Chi tiết]|
|  +--------------------------------------------------------------------------------------------------+  |
|                                                                                                        |
|  THÔNG TIN CHI TIẾT WORKSPACE ĐANG CHỌN: "Default Personal"                                             |
|  +--------------------------------------------------------------------------------------------------+  |
|  | • Định danh (ID): ws_7c81a90e-b24e-4f32-9b21-89d123456789                                       |  |
|  | • Trạng thái lưu trữ: Đám mây S3 bảo vệ (E2EE Zero-Knowledge Sync)                                |  |
|  | • Chính sách Workspace (Policy): Tối đa 500 Profiles • 10 Thành viên • Hỗ trợ Offline: Có        |  |
|  | • Bản chụp phục hồi gần nhất: 18:30 hôm nay (142 profiles sẵn sàng)                               |  |
|  | • Nhóm thành viên liên kết: 1 Owner (Không chia sẻ ra ngoài)                                     |  |
|  +--------------------------------------------------------------------------------------------------+  |
|                                                                                                        |
| ────────────────────────────────────────────────────────────────────────────────────────────────────── |
| Tổng: 3 Workspaces (620 Profiles sử dụng)                                [ + Thêm Tổ Chức ]   [ Xong ] |
+--------------------------------------------------------------------------------------------------------+
```

---

## 4. PHÂN QUYỀN & THÀNH VIÊN (ACCOUNT ROLE & RBAC)

### 4.1. Hiện Tại (Current Implementation)
- ⚠️ **Chưa có giao diện desktop**: Các contract và Tauri command (`member_list`, `member_invite`, `member_role_update`, `member_suspend`) đã sẵn sàng trong Rust backend và API, nhưng chưa có màn hình UI để người dùng thao tác.

---

### 4.2. Best UI/UX: Màn Hình Quản Lý Thành Viên & Phân Quyền (`MemberManagementDialog`)
```text
+--------------------------------------------------------------------------------------------------------+
| [👥] Quản Lý Thành Viên & Phân Quyền (RBAC)                                               [ ✕ Đóng ]   |
|      Không gian: [ 🏢 Shopee Ads Global ▼ ]  •  Đã dùng: 8 / 20 ghế thành viên                          |
| ────────────────────────────────────────────────────────────────────────────────────────────────────── |
|                                                                                                        |
|  [ 🔍 Tìm theo tên hoặc email...             ]  [ Lọc vai trò: Tất cả ▼ ]        [ + Mời Thành Viên ]  |
|                                                                                                        |
|  +--------------------------------------------------------------------------------------------------+  |
|  | Thành Viên               | Vai Trò (Role)     | Nhóm Quản Lý   | Ngày Gia Nhập| Trạng Thái| Thao Tác |
|  |--------------------------+--------------------+----------------+--------------+-----------+----------|
|  | [Avatar] Nguyen Van A    | [👑 Owner       ▼] | Toàn bộ (All)  | 2026-01-10   | ● Active  | (Chính bạn)
|  |   founder@company.com    |                    |                |              |           |          |
|  |                          |                    |                |              |           |          |
|  | [Avatar] Tran Thi B      | [🛡️ Admin       ▼] | Toàn bộ (All)  | 2026-03-15   | ● Active  | [ ⋯ Menu ]
|  |   lead_ads@company.com   |                    |                |              |           |          |
|  |                          |                    |                |              |           |          |
|  | [Avatar] Le Van C        | [⚙️ Operator    ▼] | 2 Nhóm (80 prf)| 2026-08-01   | ● Active  | [ ⋯ Menu ]
|  |   staff01@company.com    |                    |                |              |           |          |
|  |                          |                    |                |              |           |          |
|  | [Avatar] Hoang D         | [👤 Member      ▼] | 1 Nhóm (20 prf)| 2026-10-18   | ⏳ Invited| [Thu hồi]|
|  |   freelancer@gmail.com   |                    |                |              |           |          |
|  +--------------------------------------------------------------------------------------------------+  |
|                                                                                                        |
|  MA TRẬN QUYỀN HẠN THỰC THI (RBAC PREVIEW):                                                            |
|  ┌──────────────┬──────────────────┬─────────────────┬───────────────────┬──────────────────────────┐  |
|  │ Vai Trò      │ Chạy Profile (▶) │ Sửa Proxy / Cấu │ Xuất Cookie Raw   │ Mời/Xóa Thành Viên       │  |
|  │              │                  │ hình Profile    │ (Bảo mật cao)     │                          │  |
|  ├──────────────┼──────────────────┼─────────────────┼───────────────────┼──────────────────────────┤  |
|  │ 👑 Owner     │        ✅        │       ✅        │        ✅         │            ✅            │  |
|  │ 🛡️ Admin     │        ✅        │       ✅        │        ✅         │            ✅            │  |
|  │ ⚙️ Operator  │        ✅        │   ❌ Chỉ xem    │ ❌ Bị che (Masked)│            ❌            │  |
|  │ 👤 Member    │  ✅ Chỉ nhóm gán │       ❌        │        ❌         │            ❌            │  |
|  └──────────────┴──────────────────┴─────────────────┴───────────────────┴──────────────────────────┘  |
|                                                                                                        |
| ────────────────────────────────────────────────────────────────────────────────────────────────────── |
| [!] Lưu ý: Vai trò "Operator" tối ưu cho nhân viên cày việc: được mở profile nhưng không lấy cắp được data |
+--------------------------------------------------------------------------------------------------------+
```

### 4.3. Dialog Mời Thành Viên Mới (Sub-Modal `InviteMemberDialog`)
```text
+------------------------------------------------------------+
| [ + ] Mời Thành Viên Vào Workspace             [ ✕ ]       |
| ────────────────────────────────────────────────────────── |
| Email người nhận:                                          |
| +--------------------------------------------------------+ |
| | staff.marketing@gmail.com                              | |
| +--------------------------------------------------------+ |
|                                                            |
| Chọn vai trò (Role):                                       |
| ( ) 🛡️ Admin     - Toàn quyền quản trị profile và proxy    |
| (•) ⚙️ Operator  - Chỉ được Mở/Chạy profile được phân công |
| ( ) 👤 Member    - Thành viên xem thông tin cơ bản         |
|                                                            |
| Giới hạn phạm vi nhóm Profile (Scope):                     |
| [x] Tất cả các nhóm hiện tại                               |
| [ ] Hoặc chỉ cho phép truy cập:                            |
|     [x] Nhóm: Facebook Ads VN   (40 profiles)              |
|     [ ] Nhóm: TikTok Seller     (120 profiles)             |
|                                                            |
| Thời hạn lời mời: [ 7 ngày ▼ ]                             |
| ────────────────────────────────────────────────────────── |
| [ Hủy ]                                    [ ✉ Gửi Lời Mời ]|
+------------------------------------------------------------+
```

---

## 5. QUẢN LÝ NHÓM & PROFILE (GROUP MANAGEMENT)

### 5.1. Hiện Tại (Current Implementation — `group-management-dialog.tsx`)
```text
+--------------------------------------------------------------------------------------------------------+
| Quản lý nhóm                                                                                           |
| Tạo và quản lý các nhóm để tổ chức profile của bạn                                                     |
|                                                                                                        |
| [ Nhóm: 4 ]                                                                             [ + Tạo mới ]  |
|                                                                                                        |
| [x] Tên                              Số profiles                                             Thao tác  |
| ────────────────────────────────────────────────────────────────────────────────────────────────────── |
| [ ] 📁 Facebook Ads                     [ 120 ]                                               [✏]  [🗑] |
| [ ] 📁 TikTok Agency                     [ 85 ]                                               [✏]  [🗑] |
| [ ] 📁 Crypto Airdrop                    [ 40 ]                                               [✏]  [🗑] |
| [ ] 📁 Chưa phân nhóm                     [ 15 ]                                               [✏]  [🗑] |
|                                                                                                        |
+--------------------------------------------------------------------------------------------------------+
```

### 5.2. Nhận Xét Hiện Tại & Điểm Cần Cải Thiện
- ⚠️ Bảng danh sách quá đơn điệu, chỉ có Tên và Số đếm.
- ⚠️ Không có **màu sắc nhận diện (Color Tagging)** cho từng nhóm để dễ phân biệt trực quan.
- ⚠️ Không có cấu hình **Proxy mặc định theo nhóm** (khi tạo profile mới thuộc nhóm này, tự động ăn proxy của nhóm).
- ⚠️ Không có nút hành động hàng loạt quan trọng cho Antidetect: **Chạy tất cả (Run All)** hoặc **Dừng tất cả (Stop All)** các profile trong nhóm.
- ⚠️ Chưa có thanh tìm kiếm / lọc nhóm khi số lượng nhóm nhiều.

---

### 5.3. Best UI/UX: Màn Hình Quản Lý Nhóm Chuyên Nghiệp (`GroupManagementDialog`)
```text
+--------------------------------------------------------------------------------------------------------+
| [📁] Quản Lý Nhóm Profile (Groups & Folders)                                              [ ✕ Đóng ]   |
|      Tổ chức profile theo chiến dịch, gắn màu sắc, proxy mặc định và thực thi hàng loạt                 |
| ────────────────────────────────────────────────────────────────────────────────────────────────────── |
|                                                                                                        |
|  [ 🔍 Tìm kiếm nhóm...                     ]  [ Lọc trạng thái: Tất cả ▼ ]        [ + Tạo Nhóm Mới ]   |
|                                                                                                        |
|  +--------------------------------------------------------------------------------------------------+  |
|  | [x]| Màu | Tên Nhóm         | Profiles | Proxy Mặc Định     | Phân Quyền   | Thực Thi Nhanh| Thao Tác|
|  |----+-----+------------------+----------+--------------------+--------------+---------------+---------|
|  | [ ]| 🔵  | Facebook BM Ads  | 120 prf  | 🌐 US-Residential  | Toàn bộ (All)| [▶ Run] [⏹ Stop] [✏] [🗑]|
|  |    |     | Chiến dịch Q4    | (3 running)                  |              |               |         |
|  |    |     |                  |          |                    |              |               |         |
|  | [ ]| 🟢  | TikTok Farm VN   | 85 prf   | 🌐 VN-Viettel-4G   | 3 Operators  | [▶ Run] [⏹ Stop] [✏] [🗑]|
|  |    |     | Nuôi nick video  | (0 running)                  |              |               |         |
|  |    |     |                  |          |                    |              |               |         |
|  | [ ]| 🟣  | Crypto Whitelist | 40 prf   | 🔄 Xoay IP tự động | Chỉ Owner    | [▶ Run] [⏹ Stop] [✏] [🗑]|
|  |    |     | Săn retroactive  | (12 running)                 |              |               |         |
|  |    |     |                  |          |                    |              |               |         |
|  | [ ]| ⚪  | Chưa phân nhóm   | 15 prf   | (Không có)         | Toàn bộ (All)| [▶ Run] [⏹ Stop] [✏] [ ] |
|  +--------------------------------------------------------------------------------------------------+  |
|                                                                                                        |
|  KHI CHỌN CÁC DÒNG (BULK ACTION BAR XUẤT HIỆN DƯỚI ĐÁY):                                                |
|  +--------------------------------------------------------------------------------------------------+  |
|  |  Đã chọn: 2 nhóm (205 profiles)  │  [▶ Chạy tất cả]  [⏹ Dừng tất cả]  [🌐 Đổi Proxy]  [🗑 Xóa nhóm] |  |
|  +--------------------------------------------------------------------------------------------------+  |
|                                                                                                        |
| ────────────────────────────────────────────────────────────────────────────────────────────────────── |
| Tổng: 4 Nhóm • 260 Profiles (15 đang hoạt động)                                               [ Đóng ] |
+--------------------------------------------------------------------------------------------------------+
```

### 5.4. Dialog Tạo / Sửa Nhóm Chi Tiết (`CreateEditGroupDialog`)
```text
+------------------------------------------------------------+
| [📁] Tạo Nhóm Profile Mới                          [ ✕ ]   |
| ────────────────────────────────────────────────────────── |
| Tên nhóm: *                                                |
| +--------------------------------------------------------+ |
| | Facebook BM Ads - Q4                                   | |
| +--------------------------------------------------------+ |
|                                                            |
| Màu sắc nhận diện:                                         |
| (🔵 Xanh dương)  (🟢 Xanh lá)  (🟣 Tím)  (🟡 Vàng)  (🔴 Đỏ)|
| Mã màu tùy chỉnh: [#3B82F6            ]                    |
|                                                            |
| Proxy mặc định cho nhóm (Tùy chọn):                        |
| +--------------------------------------------------------+ |
| | [🌐 Chọn từ danh bạ Proxy / Hoặc dán Host:Port:User:Pass]| |
| +--------------------------------------------------------+ |
| [x] Tự động áp dụng proxy này cho profile mới tạo vào nhóm |
|                                                            |
| Ghi chú / Mục đích nhóm:                                   |
| +--------------------------------------------------------+ |
| | Tài khoản chạy camp quảng cáo thị trường US              | |
| +--------------------------------------------------------+ |
| ────────────────────────────────────────────────────────── |
| [ Hủy ]                                      [ Lưu Nhóm ]  |
+------------------------------------------------------------+
```

---

## 6. BẢNG TỔNG HỢP SO SÁNH & CẢI TIẾN TRỌNG TÂM

| Phân Hệ | Điểm Khác Biệt Giữa Hiện Tại & Best UI/UX | Tác Động Vận Hành & Giá Trị |
|---|---|---|
| **1. Login Page** | Loại bỏ hoàn toàn version tag `2.0/3.0`, bổ sung chỉ báo kết nối Core Local service, cảnh báo CapsLock, giữ form gọn gàng. | Người dùng không bị bối rối bởi lỗi vô hình; đăng nhập nhanh chóng, chuẩn xác. |
| **2. Quản Lý Tài Khoản** | Bổ sung đổi mật khẩu, danh sách phiên thiết bị (Active Sessions) để thu hồi từ xa, phân tách tab Hạn mức vs Bảo mật. | Nâng cao bảo mật tài khoản cho chủ doanh nghiệp; kiểm soát thiết bị đăng nhập. |
| **3. Org & Workspace** | Chuyển từ popover đơn sơ sang màn hình quản trị hoàn chỉnh: phân cấp Org -> Workspaces, xem profiles/members mỗi workspace. | Quản lý đa dự án / đa khách hàng chuyên nghiệp, không nhầm lẫn tài nguyên. |
| **4. Account Role (RBAC)**| Thiết kế màn hình thành viên với 4 vai trò chuẩn (`Owner`, `Admin`, `Operator`, `Member`). Có ma trận cấm Operator xem raw cookie. | Doanh nghiệp yên tâm thuê nhân viên / freelancer cày profile mà không sợ bị tuồn cookie/tài khoản. |
| **5. Quản Lý Nhóm** | Thêm bảng màu nhận diện, gán Proxy mặc định theo nhóm, nút chạy/dừng hàng loạt cả nhóm, tìm kiếm nhóm. | Tiết kiệm 80% thời gian thao tác hàng ngày cho các team nuôi hàng ngàn profile. |



---

## 7. BÀN ĐIỀU HÀNH SUPER ADMIN (PLATFORM ADMIN / ROOT CONSOLE)

> **Mã năng lực hợp đồng**: `CAP-WEB-004`, `CAP-ADM-001`, `CAP-REL-002`  
> **Đối tượng sử dụng**: Quản trị viên cấp cao nhất của hệ thống nền tảng BugLogin (System Owner / Platform Operator).  
> **Mục đích**: Quản lý đa tổ chức (Multi-tenancy), cấp phát hạn mức (Quota overrides), sinh License Key, giám sát hạ tầng Core & CDN Wayfern, và kích hoạt công tắc bảo vệ khẩn cấp (Emergency Kill-switch).

### 7.1. Best ASCII UI/UX: Platform Admin Console (`PlatformAdminView`)
```text
+------------------------------------------------------------------------------------------------------------------------+
| 🛡️ BUGLOGIN PLATFORM CONTROL CENTER  [SUPER ADMIN]                     [ 🔔 3 Alerts ] [ Admin: root@buglogin.com ▼ ]  |
| ────────────────────────────────────────────────────────────────────────────────────────────────────────────────────── |
|                                                                                                                        |
|  [HẠ TẦNG TOÀN CẦU]:                                                                                                  |
|  +--------------------+  +--------------------+  +--------------------+  +--------------------+  +-------------------+ |
|  | 🏢 Tổng Tổ Chức    |  | 💻 Profiles Active |  | 👥 Users Trực Tuyến|  | 🌐 Core API Gateway|  | 🚀 Wayfern CDN   | |
|  | 148 Organizations |  | 18,420 / 50,000    |  | 1,240 Đang Mở App  |  | ● 99.98% (42ms)    |  | ● v132.0.6834 (OK)| |
|  +--------------------+  +--------------------+  +--------------------+  +--------------------+  +-------------------+ |
|                                                                                                                        |
|  Tabs: [ 🏢 Quản Lý Tổ Chức (Tenants) ]  [ 🔑 Quản Lý License Key ]  [ 🛡️ Nhật Ký Hệ Thống ]  [ ⚙️ Hạ Tầng & Phiên Bản ]|
| ────────────────────────────────────────────────────────────────────────────────────────────────────────────────────── |
|                                                                                                                        |
|  [ 🔍 Tìm kiếm Org ID, Tên công ty, Email đại diện...         ]  [ Bộ lọc gói: Enterprise ▼ ]  [ + Cấp Phép Org Mới ]  |
|                                                                                                                        |
|  DANH SÁCH KHÁCH HÀNG / TỔ CHỨC TOÀN HỆ THỐNG:                                                                         |
|  +------------------------------------------------------------------------------------------------------------------+  |
|  | Org ID / Tên Tổ Chức   | Email Sở Hữu       | Gói Dịch Vụ | Profiles Cấp / Dùng| Ghế Member | Trạng Thái | Thao Tác   |  |
|  |------------------------+--------------------+-------------+--------------------+------------+------------+------------|  |
|  | org_8f12a9c4           | ceo@alphaagency.vn | ENTERPRISE  | 2,450 / 5,000      | 35 / 50    | ● Active   | [Điều chỉnh|  |
|  | Alpha Media Agency     |                    | (Custom)    | [█████░░░░░] 49%   |            |            |  Hạn mức]  |  |
|  |                        |                    |             |                    |            |            | [Tạm khóa] |  |
|  | org_33b7102e           | contact@mmostudio  | PRO TEAM    | 980 / 1,000 (98%!) | 10 / 10    | ⚠️ Gần Max | [Cấp Thêm] |  |
|  | MMO Top Studio         |                    |             | [██████████] 98%   |            |            | [Xem Audit]|  |
|  |                        |                    |             |                    |            |            |            |  |
|  | org_90e441aa           | spam@badactor.xyz  | STARTER     | 12 / 100           | 1 / 2      | ⛔ BỊ KHÓA | [Mở Khóa]  |  |
|  | Suspicious Entity      |                    |             |                    |            | (Vi phạm)  | [Xóa Vĩnh] |  |
|  +------------------------------------------------------------------------------------------------------------------+  |
|                                                                                                                        |
|  THÔNG TIN HẠ TẦNG NỀN TẢNG (PLATFORM INFRASTRUCTURE):                                                                |
|  +------------------------------------------------------------------------------------------------------------------+  |
|  | 🌐 HỆ THỐNG ĐÁM MÂY: Cloud S3 Storage: Online │ API Gateway: Online (v1.0.4)                                      |  |
|  | 📦 PHIÊN BẢN RUNTIME: Wayfern Core Engine: v132.0.6834.110 (Mới nhất) │ Cổng MCP: Sẵn sàng                        |  |
|  +------------------------------------------------------------------------------------------------------------------+  |
+------------------------------------------------------------------------------------------------------------------------+
```

### 7.2. Super Admin: Dialog Cấp Phép & Sinh License Key (`GenerateLicenseModal`)
```text
+------------------------------------------------------------------+
| 🔑 Sinh Khóa Bản Quyền Mới (License Generator)           [ ✕ ]   |
| ──────────────────────────────────────────────────────────────── |
| Tên khách hàng / Tổ chức nhận key:                               |
| +--------------------------------------------------------------+ |
| | VNG Digital Marketing Corp                                   | |
| +--------------------------------------------------------------+ |
|                                                                  |
| Gói cấp phép (Tier):                                             |
| ( ) Starter (100 prf)   ( ) Pro (1,000 prf)   (•) Enterprise     |
|                                                                  |
| Tùy chỉnh hạn mức (Custom Quotas):                               |
| • Hạn mức Profile:       [ 3,500    ] profiles                   |
| • Hạn mức Thành viên:    [ 40       ] seats                      |
| • Hạn mức Workspace:     [ 10       ] workspaces                 |
| • Thời hạn sử dụng:      [ 365 ngày ▼ ] (Hết hạn: 2027-10-19)    |
|                                                                  |
| Tính năng kích hoạt kèm theo:                                    |
| [x] Đồng bộ đám mây mã hóa (Cloud Sync Encrypted)                |
| [x] Cổng kết nối lập trình REST API & MCP Server                 |
| [x] Trình duyệt nhân Wayfern cao cấp (Spoofing Kernel)           |
|                                                                  |
| ──────────────────────────────────────────────────────────────── |
| [ Hủy Bỏ ]                             [ ⚡ Sinh Khóa & Kích Hoạt ]|
+------------------------------------------------------------------+
```

---

## 8. BÀN ĐIỀU HÀNH TRƯỞNG NHÓM (TEAM MANAGER DASHBOARD)

> **Mã năng lực hợp đồng**: `CAP-WS-001`, `CAP-PROF-003`, `CAP-MEM-001`  
> **Đối tượng sử dụng**: `Manager` (Trưởng nhóm quảng cáo, Quản lý ca trực, Team Lead).  
> **Vai trò trong RBAC**: Dưới quyền Admin/Owner nhưng trực tiếp quản lý các `Operator`. Không can thiệp hóa đơn hay xóa workspace, nhưng có thẩm quyền phân chia tài nguyên, giám sát ca làm việc và điều phối profile/proxy cho nhân viên.

### 8.1. Best ASCII UI/UX: Manager Command Center (`ManagerDashboardView`)
```text
+------------------------------------------------------------------------------------------------------------------------+
| 👥 TRUNG TÂM ĐIỀU HÀNH TRƯỞNG NHÓM  [TEAM MANAGER]                     [ Nhóm Phụ Trách: Team Ads US & Camp Black ]   |
| ────────────────────────────────────────────────────────────────────────────────────────────────────────────────────── |
|                                                                                                                        |
|  [CHỈ SỐ CA TRỰC ĐỘI NHÓM]:                                                                                           |
|  +--------------------+  +--------------------+  +--------------------+  +--------------------+                        |
|  | 💻 Profiles Quản Lý|  | 🚀 Đang Chạy Sống  |  | 👥 Nhân Viên Trực  |  | 🌐 Proxy Gán Đội   |                        |
|  | 420 Profiles       |  | 18 Cửa Sổ Browser  |  | 6 / 8 Operators    |  | 95 Proxies (92 Live|                        |
|  +--------------------+  +--------------------+  +--------------------+  +--------------------+                        |
|                                                                                                                        |
|  Tabs: [ 🖥️ Giám Sát Ca Trực (Real-time) ]  [ 📋 Ma Trận Gán Profile ]  [ 🌐 Kho Proxy Của Đội ]  [ 🔄 Bàn Giao Nhanh ] |
| ────────────────────────────────────────────────────────────────────────────────────────────────────────────────────── |
|                                                                                                                        |
|  BẢNG THEO DÕI NHÂN VIÊN ĐANG CHẠY PROFILE TRỰC TUYẾN:                                                                 |
|  +------------------------------------------------------------------------------------------------------------------+  |
|  | Nhân Viên (Operator)   | Profile Đang Chạy| Proxy Đang Dùng    | Thời Gian Mở| Tài Nguyên Máy  | Thao Tác Cưỡng Chế |  |
|  |------------------------+------------------+--------------------+-------------+-----------------+--------------------|  |
|  | [Avatar] Le Van C      | 🔵 FB_Ads_US_012 | 104.28.x.x (US)    | 01h 45m     | CPU: 8% • 420MB | [ ⏹ Dừng Profile ] |  |
|  |   staff01@company.com  | 🔵 FB_Ads_US_015 | 104.28.x.x (US)    | 00h 22m     | CPU: 5% • 380MB |                    |  |
|  |                        |                  |                    |             |                 |                    |  |
|  | [Avatar] Pham Thi D    | 🟢 TikTok_VN_088 | 115.79.x.x (VN-4G) | 03h 10m     | CPU: 12% • 610MB| [ ⏹ Dừng Profile ] |  |
|  |   staff02@company.com  |                  | (Ping cao: 420ms)  |             |                 | [ 🔄 Đổi Proxy ]   |  |
|  |                        |                  |                    |             |                 |                    |  |
|  | [Avatar] Hoang E       | (Không có)       | (Nghỉ giữa ca)     | --          | Online (Idle)   | [ Gán Thêm 20 Prf ]|  |
|  |   staff03@company.com  |                  |                    |             |                 |                    |  |
|  +------------------------------------------------------------------------------------------------------------------+  |
|                                                                                                                        |
|  TÍNH NĂNG ĐỘC QUYỀN TRƯỞNG NHÓM: BÀN GIAO PROFILE NHANH (FAST PROFILE HANDOVER):                                      |
|  +------------------------------------------------------------------------------------------------------------------+  |
|  | Nhân viên bàn giao: [ Le Van C (staff01) ▼ ]  ➜  Chuyển sang: [ Hoang E (staff03) ▼ ]                            |  |
|  | Số lượng: [ Toàn bộ 45 Profiles thuộc Nhóm FB BM Ads ]  │  [ ⚡ Thực Hiện Bàn Giao Ngay (Không Lộ Mật Khẩu) ]       |  |
|  +------------------------------------------------------------------------------------------------------------------+  |
+------------------------------------------------------------------------------------------------------------------------+
```

---

## 9. NHẬT KÝ KIỂM TOÁN AN TOÀN HỆ THỐNG (SECURITY AUDIT LOG)

> **Mã năng lực hợp đồng**: `CAP-AUD-001`, `CAP-SEC-001`  
> **Nguyên tắc cốt lõi**: Nhật ký bất biến (Append-only), không thể xóa sửa, che giấu dữ liệu nhạy cảm (Không bao giờ ghi raw cookie hay bearer token).  
> **Mục đích**: Chống đánh cắp tài nguyên, phát hiện nhân viên gian lận, điều tra sự cố rò rỉ hoặc đổi cấu hình ngoài ý muốn.

### 9.1. Best ASCII UI/UX: Màn Hình Nhật Ký Hoạt Động (`AuditLogDialog`)
```text
+------------------------------------------------------------------------------------------------------------------------+
| 📜 Nhật Ký Hoạt Động & Kiểm Toán An Toàn (Audit Trail)                                                    [ ✕ Đóng ]   |
|    Ghi nhận chi tiết mọi hành vi mở trình duyệt, thay đổi proxy, phân quyền và xuất dữ liệu                            |
| ────────────────────────────────────────────────────────────────────────────────────────────────────────────────────── |
|                                                                                                                        |
|  Bộ lọc: [ Thời gian: 7 ngày qua ▼ ]  [ Hành động: Tất cả ▼ ]  [ Thành viên: Tất cả ▼ ]  [ Mức độ: Tất cả ▼ ]          |
|  Tìm kiếm: [ 🔍 Tìm ID profile, email, địa chỉ IP...                                          ]  [ 📥 Xuất File CSV ]  |
|                                                                                                                        |
|  DANH SÁCH SỰ KIỆN GHI NHẬN:                                                                                           |
|  +------------------------------------------------------------------------------------------------------------------+  |
|  | Thời Gian (UTC+7) | Người Thực Hiện      | Hành Động (Action)  | Đối Tượng Tác Động  | Địa Chỉ IP   | Kết Quả      |  |
|  |-------------------+----------------------+---------------------+---------------------+--------------+--------------|  |
|  | 2026-10-19 20:14  | staff01@company.com  | profile.launch      | prf_9a82 (FB US 01) | 14.232.88.12 | ✅ Thành công|  |
|  | 2026-10-19 19:55  | staff02@company.com  | cookie.export_raw   | prf_11c0 (TikTok 04)| 118.69.45.2  | ⛔ TỪ CHỐI   |  |
|  |                   |                      | (Cố ý lấy cookie)   |                     |              | (Không quyền)|  |
|  | 2026-10-19 18:30  | lead_ads@company.com | member.role_update  | staff03 -> Operator | 14.232.88.12 | ✅ Thành công|  |
|  | 2026-10-19 17:10  | staff01@company.com  | proxy.rotate_ip     | prx_us_residential  | 14.232.88.12 | ✅ IP mới:   |  |
|  |                   |                      |                     |                     |              | 104.28.19.4  |  |
|  | 2026-10-19 15:02  | unknown_client       | auth.failed_password| ceo@alphaagency.vn  | 45.130.8.99  | ⚠️ KHÓA TẠM  |  |
|  |                   |                      | (Sai pass 5 lần)    |                     | (Nga)        | (15 phút)    |  |
|  +------------------------------------------------------------------------------------------------------------------+  |
|                                                                                                                        |
|  CHI TIẾT BẢN GHI SỰ KIỆN: "cookie.export_raw [TỪ CHỐI]"                                                               |
|  +------------------------------------------------------------------------------------------------------------------+  |
|  | • Actor ID: mem_44a901 (Pham Thi D - Operator)  • Workspace: ws_shopee_global                                   |  |
|  | • Resource ID: prf_11c09 (TikTok Shop Account 04)                                                                |  |
|  | • Lý do từ chối: Role 'Operator' không sở hữu quyền 'cookie.read_unmasked'. Yêu cầu cấp quyền đã bị chặn lại.  |  |
|  | • SHA-256 Checksum: e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855                          |  |
|  +------------------------------------------------------------------------------------------------------------------+  |
+------------------------------------------------------------------------------------------------------------------------+
```

---

## 10. QUẢN LÝ KHO PROXY TẬP TRUNG & KIỂM TRA TỐC ĐỘ (PROXY POOL FLEET)

> **Mã năng lực hợp đồng**: `CAP-NET-001`, `CAP-STOR-001`  
> **Mục đích**: Bổ sung phân hệ quản lý kho Proxy toàn diện, hỗ trợ đo Ping thực tế đồng thời, phát hiện Proxy chết (Dead), xoay IP theo lịch và gán hàng loạt cho các profile.

### 10.1. Best ASCII UI/UX: Proxy Manager & Batch Speed Tester (`ProxyManagerDialog`)
```text
+------------------------------------------------------------------------------------------------------------------------+
| 🌐 Quản Lý Kho Proxy & Kiểm Tra Tốc Độ Thực Tế                                                            [ ✕ Đóng ]   |
|    Kho Proxy Workspace: [ 🏢 Shopee Ads Global ▼ ]  •  Tổng: 95 Proxies (92 Hoạt Động • 3 Lỗi/Chết)                    |
| ────────────────────────────────────────────────────────────────────────────────────────────────────────────────────── |
|                                                                                                                        |
|  [ ⚡ Kiểm Tra Ping Toàn Bộ (95) ]  [ ➕ Thêm Proxy (Dán Text) ]  [ 🗑 Xóa Proxy Chết ]  [ 🔄 Đổi IP Tất Cả Proxy Xoay ] |
|                                                                                                                        |
|  [ 🔍 Tìm host, port, quốc gia, tag...     ]  [ Giao thức: Tất cả ▼ ]  [ Quốc gia: Tất cả ▼ ]  [ Nhóm gán: Tất cả ▼ ]  |
|                                                                                                                        |
|  +------------------------------------------------------------------------------------------------------------------+  |
|  | [x]| Quốc Gia| Địa Chỉ Proxy (Host:Port) | Giao Thức| Tốc Độ Ping | Đổi IP (Rotation)  | Gán Profile | Thao Tác     |  |
|  |----+---------+---------------------------+----------+-------------+--------------------+-------------+--------------|  |
|  | [ ]| 🇺🇸 US   | 104.28.192.15:8080        | SOCKS5   | 🟢 145 ms   | Cố định (Static)   | 12 Profiles | [Test] [✏][🗑]|  |
|  | [ ]| 🇻🇳 VN   | 115.79.24.8:9055          | HTTP     | 🟢 38 ms    | 🔄 Còn 04m 12s     | 8 Profiles  | [Đổi IP] [✏] |  |
|  | [ ]| 🇸🇬 SG   | 139.180.201.4:1080        | SOCKS5   | 🟡 280 ms   | Cố định (Static)   | 4 Profiles  | [Test] [✏][🗑]|  |
|  | [ ]| 🇩🇪 DE   | 194.87.110.2:8888         | HTTP     | 🔴 TIMEOUT  | ⚠️ Không phản hồi  | 0 Profiles  | [Thử lại][🗑]|  |
|  +------------------------------------------------------------------------------------------------------------------+  |
|                                                                                                                        |
|  HỘP THOẠI DÁN BATCH PROXY THÔNG MINH (QUICK PASTE MODAL SẴN CÓ):                                                      |
|  Định dạng hỗ trợ tự động: Host:Port | Host:Port:User:Pass | Protocol://User:Pass@Host:Port                           |
|  +------------------------------------------------------------------------------------------------------------------+  |
|  | us-proxy.oxylabs.io:8001:customer1:pass123                                                                       |  |
|  | socks5://operator:secret@115.79.24.8:9055                                                                         |  |
|  +------------------------------------------------------------------------------------------------------------------+  |
+------------------------------------------------------------------------------------------------------------------------+
```

---

## 11. THƯ VIỆN TIỆN ÍCH MỞ RỘNG & COOKIE AN TOÀN (EXTENSIONS & COOKIE VAULT)

> **Mã năng lực hợp đồng**: `CAP-COOK-001`, `extension_group_id`  
> **Mục đích**: Quản lý gói Extension cài sẵn (Extension Bundles) cho nhóm profile và nhập/xuất cookie Netscape/JSON được kiểm soát chặt chẽ.

### 11.1. Best ASCII UI/UX: Quản Lý Tiện Ích Mở Rộng Tập Trung (`ExtensionManagerDialog`)
```text
+--------------------------------------------------------------------------------------------------------+
| 🧩 Quản Lý Gói Tiện Ích Mở Rộng (Extension Bundles)                                       [ ✕ Đóng ]   |
|    Cài đặt một lần, tự động kích hoạt đồng bộ vào hàng loạt Profile theo nhóm                          |
| ────────────────────────────────────────────────────────────────────────────────────────────────────── |
|                                                                                                        |
|  Gói Extension Đang Chọn: [ 📦 Gói MMO & E-Commerce Pro ▼ ]               [ + Cài Tiện Ích Mới ]       |
|                                                                                                        |
|  DANH SÁCH TIỆN ÍCH TRONG GÓI NÀY:                                                                     |
|  +--------------------------------------------------------------------------------------------------+  |
|  | Icon | Tên Tiện Ích            | Phiên Bản| Nguồn Cài Đặt         | Trạng Thái| Thao Tác Hàng Loạt |  |
|  |------+-------------------------+----------+-----------------------+-----------+--------------------|  |
|  | 🦊   | MetaMask                | v12.4.1  | Chrome Web Store (ID) | [x] Bật   | [ Cập nhật bản mới]|  |
|  | 🍪   | Cookie-Editor           | v2.1.0   | Tải file .CRX cục bộ  | [x] Bật   | [ Cấu hình sẵn ]   |  |
|  | 📊   | Meta Pixel Helper       | v3.1.2   | Chrome Web Store (ID) | [x] Bật   | [ Gỡ khỏi gói ]    |  |
|  | 🛡️   | uBlock Origin           | v1.58.0  | Chrome Web Store (ID) | [ ] Tắt   | [ Gỡ khỏi gói ]    |  |
|  +--------------------------------------------------------------------------------------------------+  |
|                                                                                                        |
|  ÁP DỤNG GÓI EXTENSION NÀY CHO CÁC NHÓM PROFILE:                                                       |
|  [x] Nhóm: Facebook BM Ads (120 profiles)                                                              |
|  [x] Nhóm: TikTok Farm VN  (85 profiles)                                                               |
|  [ ] Nhóm: Crypto Whitelist (40 profiles)                                                              |
|                                                                                                        |
| ────────────────────────────────────────────────────────────────────────────────────────────────────── |
| [ + Tải File .CRX Cục Bộ ]             [ Nhập Link Chrome Web Store ]         [ Lưu Cấu Hình Gói ]     |
+--------------------------------------------------------------------------------------------------------+
```

---

## 12. BẢNG TỔNG KẾT TOÀN DIỆN CÁC PHÂN HỆ BUGLOGIN

| Phân Hệ | Cấp Bậc / Vai Trò | Trọng Tâm UI/UX | Giá Trị Cốt Lõi |
|---|---|---|---|
| **1. Login Page** | Mọi người dùng | Nhập liệu tập trung, phát hiện trạng thái Core Service, cảnh báo CapsLock. | 100% không vướng lỗi vô hình, vào việc nhanh. |
| **2. Quản Lý Tài Khoản** | Cá nhân người dùng | Thẻ tài nguyên, đổi mật khẩu, quản lý phiên thiết bị đang online. | Bảo mật thông tin đăng nhập, tránh bị dùng trộm. |
| **3. Org & Workspace** | Chủ sở hữu & Doanh nghiệp | Phân chia môi trường riêng biệt giữa các công ty / phòng ban. | Cách ly dữ liệu hoàn hảo, không trộn lẫn profile. |
| **4. Thành Viên & RBAC** | Admin & Nhân sự | Phân quyền 4 cấp (`Owner`, `Admin`, `Operator`, `Member`). | Bảo vệ tài sản: Operator cày việc không thể trộm cookie. |
| **5. Quản Lý Nhóm** | Vận hành hằng ngày | Gắn màu, gán proxy mặc định, chạy/dừng hàng loạt cả nhóm. | Tiết kiệm 80% thao tác tay cho các trại nuôi tài khoản. |
| **6. Super Admin (Platform)** | Quản trị nền tảng | Quản lý đa Tenant, cấp quota, sinh License Key, giám sát Wayfern. | Điều hành toàn bộ khách hàng và hạ tầng dịch vụ. |
| **7. Team Manager** | Trưởng nhóm / Ca trưởng | Giám sát Operator đang chạy profile gì, bàn giao 50 profile trong 1-click. | Kiểm soát năng suất và điều phối tài nguyên theo ca. |
| **8. Security Audit Log** | Giám sát tuân thủ | Bảng ghi bất biến: ai mở profile, ai đổi proxy, ai cố xuất cookie. | Bằng chứng rõ ràng khi có sự cố nội bộ. |
| **9. Kho Proxy & Ping Test** | Kỹ thuật mạng | Kiểm tra tốc độ thực tế đồng thời, phát hiện proxy chết, xoay IP. | Đảm bảo trình duyệt luôn có mạng sạch trước khi mở. |
| **10. Extension & Cookie Vault** | Tự động hóa & Tool | Cài sẵn bộ tiện ích theo nhóm, nạp/xuất cookie an toàn. | Chuẩn bị môi trường làm việc nhanh chóng, đồng nhất. |



---

## 13. CƠ CHẾ OWNER ORG / WORKSPACE: TẠO TÀI KHOẢN, PHÂN QUYỀN & QUẢN TRỊ VÒNG ĐỜI NHÂN SỰ

> **Mã năng lực hợp đồng**: `CAP-ORG-001`, `CAP-WS-001`, `CAP-MEM-001`, `CAP-ID-001`, `CAP-SEC-001`  
> **Đối tượng vận hành**: `Organization Owner` (Chủ doanh nghiệp) và `Workspace Admin` (Quản trị viên môi trường).

---

### 13.1. Phân Cấp Quyền Lực: Org Owner vs. Workspace Admin

```text
  ┌────────────────────────────────────────────────────────────────────────┐
  │ 👑 ORGANIZATION OWNER (Chủ Doanh Nghiệp / Chủ Tài Khoản Thanh Toán)   │
  │ • Nắm gói dịch vụ, hạn mức tổng (Total Slots, Max Seats, Storage)       │
  │ • Tạo / Phân bổ hạn mức / Xóa các Workspaces                          │
  │ • Quản lý danh bạ nhân sự toàn công ty (Tạo tài khoản, Khóa nhân viên)│
  │ • Thiết lập chính sách bảo mật bắt buộc toàn công ty (DLP, 2FA)        │
  └───────────────────────────────────┬────────────────────────────────────┘
                                      │
          ┌───────────────────────────┴───────────────────────────┐
          ▼                                                       ▼
┌───────────────────────────────────────┐   ┌───────────────────────────────────────┐
│ 🏢 WORKSPACE 1: "Shopee Global Ads"   │   │ 🏢 WORKSPACE 2: "TikTok Affiliate"    │
│ Quản trị: 🛡️ Workspace Admin A        │   │ Quản trị: 🛡️ Workspace Admin B        │
│ • Quản lý Profile, Proxy nội bộ       │   │ • Quản lý Profile, Proxy nội bộ       │
│ • Gán việc cho Team Manager & Operator│   │ • Gán việc cho Team Manager & Operator│
│ ❌ Không can thiệp sang Workspace 2   │   │ ❌ Không can thiệp sang Workspace 1   │
└───────────────────────────────────────┘   └───────────────────────────────────────┘
```

---

### 13.2. Bàn Điều Hành Chủ Tổ Chức (Org Owner Command Center — `OrgManagementCenter`)

```text
+------------------------------------------------------------------------------------------------------------------------+
| 👑 TRUNG TÂM QUẢN TRỊ DOANH NGHIỆP (ORGANIZATION SETTINGS)                                                [ ✕ Đóng ]   |
|    Tổ Chức: [ 🏢 Alpha Media Group  ▼ ]  •  Gói: PRO ENTERPRISE  •  Thời Hạn: 2027-12-31 (Còn 428 ngày)                |
| ────────────────────────────────────────────────────────────────────────────────────────────────────────────────────── |
|                                                                                                                        |
|  [HẠN MỨC TÀI NGUYÊN TỔ CHỨC]:                                                                                         |
|  • Profiles: 2,450 / 5,000 slots (49%)    • Ghế Thành Viên: 35 / 50 seats (70%)    • Workspaces: 4 / 10 Active        |
|  [██████████░░░░░░░░░░]                   [██████████████░░░░░░]                   [████░░░░░░░░░░░░░░░░]              |
|                                                                                                                        |
|  Tabs: [ 👥 Quản Lý Tài Khoản Nhân Sự (35) ]  [ 🏢 Quản Lý Workspaces (4) ]  [ 🛡️ Chính Sách Bảo Mật ]  [ 💳 Hóa Đơn & Gói]|
| ────────────────────────────────────────────────────────────────────────────────────────────────────────────────────── |
|                                                                                                                        |
|  [ 🔍 Tìm theo Tên, Email, Chức vụ...          ]  [ Lọc Workspace: Tất cả ▼ ]  [ ➕ Tạo Tài Khoản ]  [ ✉ Mời Qua Email ]|
|                                                                                                                        |
|  DANH BẠ NHÂN SỰ TOÀN TỔ CHỨC:                                                                                         |
|  +------------------------------------------------------------------------------------------------------------------+  |
|  | Nhân Viên & Email        | Quyền Org     | Workspaces Được Vào | Nhóm Profile Phụ Trách| Trạng Thái| Thao Tác Quản Trị|  |
|  |--------------------------+---------------+---------------------+-----------------------+-----------+------------------|  |
|  | [Avatar] Nguyen Van A    | 👑 Owner      | Toàn bộ (4/4 WS)    | Toàn quyền (All)      | ● Active  | (Chính bạn)      |  |
|  |   owner@alphamedia.vn    |               |                     |                       |           |                  |  |
|  | [Avatar] Tran Thi B      | 🛡️ Org Admin  | 3 Workspaces        | Toàn quyền WS được gán| ● Active  | [Phân quyền] [⋯] |  |
|  |   lead_hr@alphamedia.vn  |               |                     |                       |           |                  |  |
|  | [Avatar] Le Van C        | 👥 Manager    | WS: Shopee Global   | Nhóm FB Ads (45 prf)  | ● Active  | [Phân quyền] [⋯] |  |
|  |   staff01@alphamedia.vn  |               |                     |                       |           |                  |  |
|  | [Avatar] Hoang Van D     | ⚙️ Operator   | WS: TikTok Studio   | Nhóm Nuôi Kênh (20)   | ● Active  | [Phân quyền] [⋯] |  |
|  |   staff02@alphamedia.vn  |               |                     |                       |           |                  |  |
|  | [Avatar] Pham Thi E      | ⚙️ Operator   | (Chưa gán)          | Không có              | ⛔ TẠM KHÓA| [Mở Khóa] [Xóa]  |  |
|  |   offboard@gmail.com     |               |                     | (Đã đình chỉ công tác)|           |                  |  |
|  +------------------------------------------------------------------------------------------------------------------+  |
|                                                                                                                        |
|  MENU THAO TÁC NHANH CHO MỖI NHÂN VIÊN ([⋯]):                                                                          |
|  │ 🔑 Đặt lại mật khẩu khởi tạo  │  🛑 Đăng xuất cưỡng chế mọi máy  │  ⛔ Khóa tài khoản ngay  │  🗑 Xóa khỏi công ty  │  |
+------------------------------------------------------------------------------------------------------------------------+
```

---

### 13.3. Phương Thức 1: Tạo Trực Tiếp Tài Khoản Cho Nhân Viên (Direct Sub-Account Provisioning)
> **Giải quyết bài toán thực tế**: Chủ doanh nghiệp muốn cấp tài khoản ngay cho nhân viên vào ca mà không cần chờ nhân viên check email hoặc đăng ký lằng nhằng.

```text
+----------------------------------------------------------------------+
| ➕ Cấp Tài Khoản Mới Trực Tiếp Cho Nhân Sự                   [ ✕ ]   |
| ──────────────────────────────────────────────────────────────────── |
| 1. THÔNG TIN ĐỊNH DANH ĐĂNG NHẬP:                                    |
| Tên hiển thị: *                 Email đăng nhập: *                   |
| +-----------------------------+ +----------------------------------+ |
| | Le Van Nam                  | | nam.staff04@alphamedia.vn        | |
| +-----------------------------+ +----------------------------------+ |
|                                                                      |
| Mật khẩu ban đầu: *                                                  |
| +--------------------------------------------------------------+---+ |
| | Alpha@2026!Pass                                              | 👁 | |
| +--------------------------------------------------------------+---+ |
| [!] Mật khẩu khởi tạo được mã hóa Argon2id trực tiếp vào hệ thống     |
|                                                                      |
| 2. GÁN VÀO KHÔNG GIAN LÀM VIỆC (WORKSPACE):                          |
| [x] Shopee Global Ads  ─── Vai trò: [ ⚙️ Operator (Nhân Viên)    ▼ ]  |
| [ ] TikTok Affiliate   ─── Vai trò: [ -- Không tham gia --       ▼ ]  |
| [ ] Crypto Airdrop     ─── Vai trò: [ -- Không tham gia --       ▼ ]  |
|                                                                      |
| 3. GIỚI HẠN NHÓM PROFILE ĐƯỢC PHÉP TRUY CẬP:                         |
| ( ) Toàn bộ Profile trong Workspace                                  |
| (•) Chỉ định nhóm cụ thể:                                            |
|     [x] 🔵 Facebook BM Ads Q4 (120 profiles)                         |
|     [ ] 🟢 TikTok Farm VN     (85 profiles)                          |
|                                                                      |
| 4. BẢO MẬT & CHỐNG RÒ RỈ DỮ LIỆU (DLP POLICIES):                     |
| [x] CHẶN XUẤT RAW COOKIE: Nhân viên không thể copy/export cookie     |
| [x] ẨN MẬT KHẨU PROXY: Chỉ cho phép dùng mạng, không xem pass proxy  |
| [x] CHẶN XÓA PROFILE: Nhân viên không có quyền xóa dữ liệu           |
| ──────────────────────────────────────────────────────────────────── |
| [ Hủy ]                                      [ ⚡ Tạo Tài Khoản Ngay ]|
+----------------------------------------------------------------------+
```

---

### 13.4. Modal Phân Quyền Chi Tiết Đa Tầng (Granular Permission Matrix Modal)

```text
+--------------------------------------------------------------------------------------------------------+
| 🛡️ Phân Quyền Chi Tiết Cho Nhân Viên: [ Le Van Nam (nam.staff04) ]                       [ ✕ Đóng ]   |
| ────────────────────────────────────────────────────────────────────────────────────────────────────── |
|  Cấp Bậc Toàn Tổ Chức: [ ⚙️ Operator (Nhân Viên Vận Hành) ▼ ]                                         |
|                                                                                                        |
|  BẢNG MA TRẬN QUYỀN TRÊN TỪNG WORKSPACE & TÀI NGUYÊN:                                                  |
|  +--------------------------------------------------------------------------------------------------+  |
|  | Không Gian Làm Việc | Vai Trò Cụ Thể | Nhóm Profile Cho Phép  | Mở Trình Duyệt| Xem Cookie| Sửa Proxy|  |
|  |---------------------+----------------+------------------------+---------------+-----------+----------|  |
|  | 🏢 Shopee Global Ads| Operator       | 🔵 FB BM Ads (120 prf) |      ✅       | ❌ Bị Che | ❌ Bị Cấm|  |
|  | 🏢 TikTok Studio    | (Không vào)    | --                     |      --       |    --     |    --    |  |
|  | 🏢 Crypto Airdrop   | (Không vào)    | --                     |      --       |    --     |    --    |  |
|  +--------------------------------------------------------------------------------------------------+  |
|                                                                                                        |
|  CHI TIẾT PHÂN QUYỀN TRÊN TÀI NGUYÊN (RESOURCE GRANTS):                                                |
|  • Quyền Profile: [x] Mở/Chạy (Execute)   [ ] Tạo mới (Create)   [ ] Đổi tên/Sửa (Update)  [ ] Xóa (Delete)|
|  • Quyền Proxy:   [x] Dùng mạng (Execute) [ ] Thêm Proxy (Create) [ ] Đổi Proxy (Update)   [ ] Xóa (Delete)|
|  • Quyền Cookie:  [x] Đọc Cookie đã che   [ ] Xuất Cookie thô (Unmasked - Yêu cầu Admin)                   |
|                                                                                                        |
| ────────────────────────────────────────────────────────────────────────────────────────────────────── |
| [ Hủy Thay Đổi ]                                                               [ 💾 Lưu Cấu Hình Quyền ]|
+--------------------------------------------------------------------------------------------------------+
```

---

### 13.5. Quy Trình Thu Hồi & Xử Lý Khi Nhân Viên Nghỉ Việc (Offboarding & Revocation Flow)

```text
+----------------------------------------------------------------------+
| ⚠️ Xử Lý Đình Chỉ / Thu Hồi Nhân Sự: [ Le Van C ]            [ ✕ ]   |
| ──────────────────────────────────────────────────────────────────── |
| Bạn đang chuẩn bị ngừng quyền truy cập của nhân sự này khỏi công ty. |
|                                                                      |
| HÀNH ĐỘNG HỆ THỐNG SẼ THỰC HIỆN TỰ ĐỘNG:                             |
| 1. [x] ĐÓNG TỨC THÌ mọi cửa sổ trình duyệt nhân viên đang mở         |
| 2. [x] THU HỒI TOKEN & ĐĂNG XUẤT phiên làm việc trên mọi thiết bị    |
| 3. [x] CHUYỂN TOÀN BỘ 45 Profile nhân viên đang giữ sang cho:        |
|        Người nhận bàn giao: [ Hoang E (Manager)                   ▼ ] |
| 4. [x] KHÓA ĐĂNG NHẬP vĩnh viễn (Tài khoản chuyển sang trạng thái    |
|        'Suspended' - Không thể đăng nhập lại)                        |
|                                                                      |
| Ghi chú lý do kiểm toán (Audit Reason):                              |
| +------------------------------------------------------------------+ |
| | Hết hạn hợp đồng thử việc ngày 2026-10-19. Bàn giao sang team 2. | |
| +------------------------------------------------------------------+ |
| ──────────────────────────────────────────────────────────────────── |
| [ Hủy Bỏ ]                                   [ ⛔ Cưỡng Chế Thu Hồi ]|
+----------------------------------------------------------------------+
```

---

### 13.6. Tóm Tắt Giá Trị Vận Hành Cho Doanh Nghiệp (Business Impact)

1. **Khép kín 100% tài nguyên**: Mọi Profile, Proxy, Extension, Cookie đều là tài sản của Tổ Chức (Organization), gắn liền với bản quyền Owner.
2. **Không sợ mất trộm tài khoản**: Nhân viên chỉ được mở trình duyệt đã gắn sẵn proxy để làm việc. Không có cách nào lấy được Cookie Raw hay mật khẩu proxy mang ra ngoài.
3. **Chuyển giao ca trong 5 giây**: Nhân viên nghỉ ốm hay nghỉ việc, Owner/Manager chỉ cần 1 click là chuyển toàn bộ Profile sang nhân sự khác mà không mất 1 dòng lịch sử duyệt web nào.


---

## 14. BỘ ASCII WIREFRAMES TOÀN DIỆN CHO MỌI THAO TÁC (ACTIONS & MODALS) & CƠ CHẾ LỜI MỜI KHÔNG DÙNG EMAIL

---

### 14.1. Cơ Chế Lời Mời Không Cần Email (No-SMTP Invitation Suite)

> **Bối cảnh kỹ thuật**: Theo quyết định tại `docs/superpowers/specs/2026-10-14-recovery-delivery-unsupported-design.md`, bản phát hành BugLogin hiện tại chưa cấu hình máy chủ gửi email ra ngoài (No Outbound SMTP).  
> **Giải pháp chuẩn hóa**: Áp dụng cơ chế **Mã Mời (Invite Code) + Link Mời (Deep Link)** kết hợp **Hộp Thư Nhận Lời Mời Trong App (In-App Pending Invites)**.

#### Thao Tác 1A: Tạo Lời Mời & Sinh Link/Mã Mời (`CreateInviteModal`)
```text
+----------------------------------------------------------------------+
| ✉ Tạo Lời Mời Tham Gia Workspace                             [ ✕ ]   |
| ──────────────────────────────────────────────────────────────────── |
| Workspace: [ 🏢 Shopee Global Ads                               ▼ ]  |
|                                                                      |
| Email định danh người nhận:                                          |
| +------------------------------------------------------------------+ |
| | freelancer.marketing@gmail.com                                   | |
| +------------------------------------------------------------------+ |
|                                                                      |
| Vai trò bổ nhiệm:                                                    |
| ( ) 🛡️ Workspace Admin   ( ) 👥 Team Manager   (•) ⚙️ Operator       |
|                                                                      |
| Phạm vi nhóm Profile được gán:                                       |
| [x] 🔵 Facebook BM Ads Q4 (120 profiles)                             |
| [ ] 🟢 TikTok Farm VN     (85 profiles)                              |
|                                                                      |
| Thời hạn mã mời: [ 7 ngày (Hết hạn lúc: 2026-10-26 20:00)        ▼ ] |
|                                                                      |
| ──────────────────────────────────────────────────────────────────── |
| ⚡ KẾT QUẢ MÃ MỜI ĐÃ TẠO (GỬI TRỰC TIẾP QUA TELEGRAM / ZALO / CHAT):   |
|                                                                      |
| Mã Mời (Invite Code):                                                |
| +--------------------------------------------------------------+---+ |
| | INV-8F92-A4B1-2026                                           | 📋| |
| +--------------------------------------------------------------+---+ |
|                                                                      |
| Link Tham Gia Trực Tiếp (Deep Link):                                 |
| +--------------------------------------------------------------+---+ |
| | buglogin://invite?code=INV-8F92-A4B1-2026                    | 📋| |
| +--------------------------------------------------------------+---+ |
|                                                                      |
| [!] Người nhận chỉ cần mở BugLogin Desktop, nhập mã này hoặc click link|
| ──────────────────────────────────────────────────────────────────── |
| [ Xong / Đóng ]                           [ 📋 Sao Chép Toàn Bộ Tin ] |
+----------------------------------------------------------------------+
```

#### Thao Tác 1B: Hộp Thư Nhận Lời Mời Trong App Của Người Được Mời (`PendingInvitesModal`)
*Khi người nhận mở BugLogin Desktop app, trên thanh tiêu đề xuất hiện chuông thông báo có chấm đỏ `🔔 [1]`.*
```text
+----------------------------------------------------------------------+
| 🔔 Lời Mời Tham Gia Workspace Đang Chờ Xử Lý (1)             [ ✕ ]   |
| ──────────────────────────────────────────────────────────────────── |
|                                                                      |
|  +----------------------------------------------------------------+  |
|  | 🏢 ALPHA MEDIA GROUP  ➜  Workspace: "Shopee Global Ads"       |  |
|  |                                                                |  |
|  | • Người mời: Nguyen Van A (owner@alphamedia.vn)                |  |
|  | • Vai trò bổ nhiệm: ⚙️ Operator (Nhân Viên Vận Hành)          |  |
|  | • Quyền hạn: Mở 120 Profiles thuộc nhóm "Facebook BM Ads Q4"   |  |
|  | • Hạn chót xác nhận: Còn 5 ngày (đến 2026-10-26)               |  |
|  |                                                                |  |
|  | [ ✕ Từ Chối Lời Mời ]                     [ ✅ Chấp Nhận & Vào ]|  |
|  +----------------------------------------------------------------+  |
|                                                                      |
|  Bạn có mã mời nhận từ bên ngoài? [ ✉ Nhập Mã Mời Bằng Tay Tại Đây ]  |
+----------------------------------------------------------------------+
```

#### Thao Tác 1C: Nhập Mã Mời Thủ Công (`RedeemInviteCodeModal`)
*Dành cho trường hợp người dùng nhận mã code qua tin nhắn ngoài app.*
```text
+----------------------------------------------------------------------+
| ✉ Nhập Mã Mời Tham Gia Workspace                             [ ✕ ]   |
| ──────────────────────────────────────────────────────────────────── |
| Dán mã mời nhận được từ Quản trị viên của bạn:                       |
| +--------------------------------------------------------------+---+ |
| | INV-8F92-A4B1-2026                                           | 📋| |
| +--------------------------------------------------------------+---+ |
|                                                                      |
| 🔎 KẾT QUẢ XÁC THỰC MÃ MỜI:                                          |
| • Tổ chức: 🏢 Alpha Media Group                                      |
| • Không gian: 🏢 Shopee Global Ads                                   |
| • Vai trò của bạn: ⚙️ Operator                                       |
| • Trạng thái mã: ● Hợp lệ (Hết hạn sau 5 ngày)                      |
|                                                                      |
| ──────────────────────────────────────────────────────────────────── |
| [ Hủy ]                                        [ 🚀 Tham Gia Ngay ]  |
+----------------------------------------------------------------------+
```

---

### 14.2. Chỉnh Sửa Workspace (`EditWorkspaceModal`)

```text
+----------------------------------------------------------------------+
| ⚙️ Chỉnh Sửa Không Gian Làm Việc (Workspace Settings)         [ ✕ ]   |
| ──────────────────────────────────────────────────────────────────── |
| Tên Workspace: *                                                     |
| +------------------------------------------------------------------+ |
| | Shopee Global Ads                                                | |
| +------------------------------------------------------------------+ |
|                                                                      |
| Thuộc Tổ Chức (Parent Organization):                                 |
| +------------------------------------------------------------------+ |
| | 🏢 Alpha Media Group (org_8f12a9c4)                           ▼ | |
| +------------------------------------------------------------------+ |
|                                                                      |
| Hạn mức phân bổ cho Workspace (Workspace Policy):                    |
| • Hạn mức Profile tối đa:    [ 500      ] profiles                   |
| • Hạn mức Thành viên tối đa:  [ 10       ] members                    |
|                                                                      |
| Tùy chọn vận hành:                                                   |
| [x] Cho phép mở Profile ngoại tuyến khi mất mạng (Offline Entitled)  |
| [x] Đồng bộ tự động đám mây mã hóa (E2EE Cloud Sync)                 |
|                                                                      |
| ──────────────────────────────────────────────────────────────────── |
| 🛑 KHU VỰC NGUY HIỂM (DANGER ZONE):                                  |
| [ 🗑 Xóa / Giải Tán Workspace Này... ]                                |
| ──────────────────────────────────────────────────────────────────── |
| [ Hủy Thay Đổi ]                                   [ 💾 Lưu Cập Nhật ]|
+----------------------------------------------------------------------+
```

---

### 14.3. Xác Nhận Xóa Workspace Khẩn Cấp (`DeleteWorkspaceModal`)

```text
+----------------------------------------------------------------------+
| ⚠️ CẢNH BÁO: XÓA KHÔNG GIAN LÀM VIỆC                         [ ✕ ]   |
| ──────────────────────────────────────────────────────────────────── |
| Bạn đang chuẩn bị xóa vĩnh viễn Workspace: "Shopee Global Ads"       |
|                                                                      |
| TÀI NGUYÊN BỊ ẢNH HƯỞNG:                                             |
| • 320 Profiles trình duyệt đang lưu trữ trong Workspace này          |
| • 8 Thành viên và Operator sẽ bị thu hồi quyền truy cập              |
| • Mọi Proxy gán riêng cho Workspace này sẽ được trả về kho tự do     |
|                                                                      |
| Phương án xử lý 320 Profiles:                                        |
| (•) Chuyển toàn bộ Profile vào Kho Lưu Trữ (Archive) để giữ dữ liệu  |
| ( ) Xóa sạch vĩnh viễn không thể khôi phục                           |
|                                                                      |
| Để xác nhận, vui lòng nhập chính xác tên Workspace bên dưới:         |
| [ Shopee Global Ads                                                ] |
| ──────────────────────────────────────────────────────────────────── |
| [ Hủy Bỏ ]                  [ 💣 Tôi hiểu rủi ro, Xóa Workspace Này ]|
+----------------------------------------------------------------------+
```

---

### 14.4. Chỉnh Sửa Thành Viên & Điều Chỉnh Vai Trò (`EditMemberModal`)

```text
+----------------------------------------------------------------------+
| 👤 Chỉnh Sửa Thành Viên: [ Le Van C (staff01) ]              [ ✕ ]   |
| ──────────────────────────────────────────────────────────────────── |
| Email: staff01@alphamedia.vn  •  Ngày tham gia: 2026-08-01           |
|                                                                      |
| Vai trò trong Workspace này:                                         |
| [ ⚙️ Operator (Nhân Viên Vận Hành)                                ▼ ] |
|                                                                      |
| Trạng thái tài khoản:                                                |
| (•) ● Đang hoạt động (Active)    ( ) ⛔ Tạm khóa (Suspended)         |
|                                                                      |
| Danh sách nhóm Profile phụ trách:                                    |
| [x] 🔵 Facebook BM Ads Q4 (120 profiles)                             |
| [ ] 🟢 TikTok Farm VN     (85 profiles)                              |
|                                                                      |
| Chính sách an toàn & hạn mức:                                        |
| • Số Profile tối đa được mở cùng lúc: [ 3 cửa sổ ▼ ]                 |
| • [x] Che giấu Cookie thô (Không cho copy/export)                    |
| • [x] Ẩn mật khẩu Proxy                                              |
|                                                                      |
| Thao tác bảo mật khẩn cấp:                                           |
| [ 🔑 Đặt Lại Mật Khẩu Khởi Tạo ]   [ 🛑 Cưỡng Chế Đăng Xuất Mọi Máy ] |
| ──────────────────────────────────────────────────────────────────── |
| [ Hủy ]                                             [ 💾 Lưu Quyền ] |
+----------------------------------------------------------------------+
```

---

### 14.5. Đặt Lại Mật Khẩu Thành Viên Khẩn Cấp (`ResetMemberPasswordModal`)
> **Bối cảnh**: Nhân viên quên mật khẩu hoặc nghi ngờ bị lộ máy tính; Owner/Admin trực tiếp đổi mật khẩu cấp lại ngay lập tức.

```text
+----------------------------------------------------------------------+
| 🔑 Đặt Lại Mật Khẩu Cho Thành Viên                           [ ✕ ]   |
| ──────────────────────────────────────────────────────────────────── |
| Tài khoản: Le Van C (staff01@alphamedia.vn)                          |
|                                                                      |
| Mật khẩu mới được hệ thống sinh tự động:                             |
| +--------------------------------------------------------------+---+ |
| | Pass#89a2!Alpha2026                                          | 📋| |
| +--------------------------------------------------------------+---+ |
| [ 🔄 Đổi Mã Khác ]                                                   |
|                                                                      |
| Tùy chọn bảo mật:                                                    |
| [x] Bắt buộc nhân viên đổi mật khẩu ở lần đăng nhập tiếp theo        |
| [x] Đăng xuất cưỡng chế khỏi tất cả thiết bị khác ngay lập tức       |
|                                                                      |
| Ghi chú: Hãy sao chép mật khẩu trên và gửi trực tiếp cho nhân viên.  |
| ──────────────────────────────────────────────────────────────────── |
| [ Hủy ]                                   [ ⚡ Cập Nhật Mật Khẩu Mới ]|
+----------------------------------------------------------------------+
```

---

### 14.6. Chỉnh Sửa Tổ Chức (Edit Organization Modal — `EditOrgModal`)

```text
+----------------------------------------------------------------------+
| 🏢 Cài Đặt Tổ Chức (Organization Settings)                   [ ✕ ]   |
| ──────────────────────────────────────────────────────────────────── |
| Tên Tổ Chức: *                                                       |
| +------------------------------------------------------------------+ |
| | Alpha Media Group                                                | |
| +------------------------------------------------------------------+ |
|                                                                      |
| Email nhận hóa đơn & thông báo bảo mật:                              |
| +------------------------------------------------------------------+ |
| | billing@alphamedia.vn                                            | |
| +------------------------------------------------------------------+ |
|                                                                      |
| CHÍNH SÁCH BẢO MẬT BẮT BUỘC TOÀN CÔNG TY (SECURITY POLICIES):        |
| [x] Bắt buộc xác thực 2 bước (2FA) đối với Admin & Manager           |
| [x] Tự động đăng xuất tài khoản sau: [ 30 phút ▼ ] không tương tác   |
| [x] Chặn toàn bộ hành vi xuất Cookie thô đối với vai trò Operator    |
| [x] Ghi nhận nhật ký Audit đầy đủ mọi thao tác mở trình duyệt        |
|                                                                      |
| ──────────────────────────────────────────────────────────────────── |
| 🛑 KHU VỰC CHUYỂN GIAO QUYỀN LỰC:                                    |
| [ 👑 Chuyển Nhượng Quyền Sở Hữu Tổ Chức (Transfer Ownership)... ]     |
| ──────────────────────────────────────────────────────────────────── |
| [ Hủy ]                                            [ 💾 Lưu Cài Đặt ]|
+----------------------------------------------------------------------+
```

---

### 14.7. Chuyển Nhượng Quyền Sở Hữu Tổ Chức (`TransferOwnershipModal`)

```text
+----------------------------------------------------------------------+
| 👑 Chuyển Nhượng Quyền Sở Hữu Tổ Chức (Transfer Ownership)   [ ✕ ]   |
| ──────────────────────────────────────────────────────────────────── |
| ⚠️ CẢNH BÁO QUAN TRỌNG:                                              |
| Bạn đang thực hiện chuyển nhượng vai trò OWNER của "Alpha Media Group"|
| cho một người khác. Sau khi hoàn tất:                                |
| • Bạn sẽ trở thành Quản trị viên (Admin) thông thường.               |
| • Bạn sẽ không còn quyền thanh toán, hủy gói, hoặc xóa Tổ Chức này.  |
|                                                                      |
| Chọn thành viên nhận quyền sở hữu mới:                               |
| +------------------------------------------------------------------+ |
| | [Avatar] Tran Thi B (lead_hr@alphamedia.vn) - Admin           ▼ | |
| +------------------------------------------------------------------+ |
|                                                                      |
| Nhập mật khẩu tài khoản của bạn để xác thực hành động này:           |
| +--------------------------------------------------------------+---+ |
| | •••••••••••••••••••••••••••••••••••••••••••••••••••••••••••••| 👁 | |
| +--------------------------------------------------------------+---+ |
|                                                                      |
| Để xác nhận, vui lòng gõ đúng cụm từ: "CHUYEN NHUONG SO HUU"         |
| [ CHUYEN NHUONG SO HUU                                             ] |
| ──────────────────────────────────────────────────────────────────── |
| [ Hủy Bỏ ]                                   [ 👑 Xác Nhận Bàn Giao ]|
+----------------------------------------------------------------------+
```

---

### 14.8. Tạo Mới Không Gian Làm Việc (`CreateWorkspaceModal`)

```text
+----------------------------------------------------------------------+
| ➕ Tạo Không Gian Làm Việc Mới (Create Workspace)             [ ✕ ]   |
| ──────────────────────────────────────────────────────────────────── |
| Tên Không Gian (Workspace Name): *                                   |
| +------------------------------------------------------------------+ |
| | TikTok Shop US Automation                                        | |
| +------------------------------------------------------------------+ |
|                                                                      |
| Thuộc Tổ Chức:                                                       |
| +------------------------------------------------------------------+ |
| | 🏢 Alpha Media Group (org_8f12a9c4)                           ▼ | |
| +------------------------------------------------------------------+ |
|                                                                      |
| Phân bổ hạn mức từ Tổ Chức (Workspace Policy):                       |
| • Hạn mức Profile:    [ 300      ] profiles (Còn 2,550 profiles tự do)|
| • Hạn mức Thành viên:  [ 5        ] members                           |
|                                                                      |
| Tùy chọn vận hành:                                                   |
| [x] Cho phép mở Profile ngoại tuyến khi mất mạng (Offline Entitled)  |
| [x] Tự động đồng bộ đám mây an toàn (Cloud Sync E2EE)                |
| ──────────────────────────────────────────────────────────────────── |
| [ Hủy ]                                      [ 🚀 Khởi Tạo Workspace ]|
+----------------------------------------------------------------------+
```



---

## 15. BẢNG CHECKLIST TRA CỨU API HỖ TRỢ TRIỂN KHAI (API READINESS & ADAPTATION MATRIX)

Bảng này đối soát chính xác 100% giữa **Màn hình / Nút bấm trên ASCII Wireframe** với **Mã nguồn Backend Rust / Tauri Command hiện tại**.  
Quy ước phân loại:
- 🟢 **ĐÃ CÓ API 100%**: Đã có sẵn lệnh Tauri trong `apps/desktop/src-tauri/src/lib.rs` và phương thức trong `bridge.ts`. Frontend gọi được ngay qua `invoke(...)` hoặc `api.method(...)`.
- 🟡 **CẦN SỬA / CẦN ADAPT**: Backend đã có khung hợp đồng nhưng payload/tham số cần mở rộng (ví dụ: thêm trường `color`, `appearance`) hoặc cần hàm bọc (wrapper) để frontend gọi tiện hơn.
- 🔴 **CHƯA CÓ API**: Chưa có lệnh Tauri tương ứng, cần bổ sung hàm `#[tauri::command]` vào `apps/desktop/src-tauri/src/lib.rs` trước khi nối UI.

---

### 15.1. Phân Hệ Đăng Nhập & Tài Khoản (Login & Account)

| Màn Hình / Thao Tác UI | Trạng Thái API | Tên Lệnh Tauri / Backend Method | Vị Trí Code Hiện Tại | Hướng Dẫn Kỹ Thuật / Điều Chỉnh Cần Làm |
|---|:---:|---|---|---|
| **Kiểm tra trạng thái đăng nhập** | 🟢 ĐÃ CÓ | `desktop_auth_status` | `lib.rs:184`, `bridge.ts:130` | Trả về `authenticated`, `unauthenticated`, `auth_not_applicable`. Gọi ngay. |
| **Gửi form đăng nhập (Email + Pass)** | 🟢 ĐÃ CÓ | `desktop_auth_login` | `lib.rs:185`, `bridge.ts:131` | Nhận `{ username, password }`. Gọi ngay. |
| **Ghi nhớ email (`rememberEmail`)** | 🟢 ĐÃ CÓ | Web Storage (`localStorage`) | `AuthBootstrap.tsx:26-34` | Đã chạy 100% trên React/Webview. |
| **Đăng xuất tài khoản (Logout)** | 🟢 ĐÃ CÓ | `desktop_auth_logout` | `lib.rs:186`, `bridge.ts:133` | Xóa session, phát event `DESKTOP_AUTH_STATUS_CHANGED_EVENT`. |
| **Lấy thông tin tài khoản & gói** | 🟢 ĐÃ CÓ | `account_get` | `lib.rs:82`, `bridge.ts:124` | Trả về `AccountView` (`id`, `email`, `status`, `createdAt`, v.v.). |
| **Đổi mật khẩu người dùng** | 🔴 CHƯA CÓ | `account_password_change` | *Chưa đăng ký trong lib.rs* | **Cần thêm**: Viết lệnh `#[tauri::command] async fn account_password_change(currentPassword, newPassword)` trong `lib.rs` gọi sang identity service. |
| **Xem danh sách phiên thiết bị (Sessions)**| 🟡 CẦN ADAPT | `auth.sessions.list` | `command_registry.rs:88` | Backend Rust đã có command `auth.sessions.list`, cần khai báo thêm `#[tauri::command]` trong `lib.rs` để UI gọi được. |
| **Thu hồi phiên thiết bị từ xa** | 🟡 CẦN ADAPT | `auth.sessions.revoke` | `command_registry.rs:89` | Backend Rust đã có `auth.sessions.revoke`, cần bọc thành `#[tauri::command]` trong `lib.rs`. |

---

### 15.2. Phân Hệ Tổ Chức & Không Gian Làm Việc (Org & Workspaces)

| Màn Hình / Thao Tác UI | Trạng Thái API | Tên Lệnh Tauri / Backend Method | Vị Trí Code Hiện Tại | Hướng Dẫn Kỹ Thuật / Điều Chỉnh Cần Làm |
|---|:---:|---|---|---|
| **Lấy danh sách Tổ Chức (Org List)** | 🟢 ĐÃ CÓ | `organization_list` | `lib.rs:85`, `bridge.ts:144` | Trả về `Page<Organization>`. Gọi ngay. |
| **Tạo Tổ Chức mới (Create Org)** | 🟢 ĐÃ CÓ | `organization_create` | `lib.rs:87`, `bridge.ts:146` | Nhận `{ organization, policy }`. Gọi ngay. |
| **Cập nhật Tổ Chức (Update Org)** | 🟢 ĐÃ CÓ | `organization_update` | `lib.rs:88`, `bridge.ts:147` | Nhận `{ organizationId, update }`. Gọi ngay. |
| **Lấy danh sách Workspace** | 🟢 ĐÃ CÓ | `workspace_list` | `lib.rs:89`, `bridge.ts:148` | Nhận `organizationId`. Trả về `Page<Workspace>`. Gọi ngay. |
| **Tạo Workspace mới** | 🟢 ĐÃ CÓ | `workspace_create` | `lib.rs:91`, `bridge.ts:150` | Nhận `{ organizationId, workspace }`. Gọi ngay. |
| **Chỉnh sửa Workspace (Sửa tên, policy)**| 🟢 ĐÃ CÓ | `workspace_update` | `lib.rs:92`, `bridge.ts:151` | Nhận `{ organizationId, workspaceId, update }`. Gọi ngay. |
| **Chuyển Workspace đang hoạt động** | 🟢 ĐÃ CÓ | `select_active_workspace` | `lib.rs:159`, `legacy_compat.rs`| Nhận `{ workspaceId }`. Thiết lập active context và reload state. |
| **Lấy danh sách workspace cho Switcher** | 🟢 ĐÃ CÓ | `get_available_workspaces` | `lib.rs:158`, `legacy_compat.rs`| Trả về mảng workspace gọn (`id`, `name`). Gọi ngay. |
| **Xóa / Giải tán Workspace** | 🔴 CHƯA CÓ | `workspace_delete` | *Chưa có trong registry/lib.rs*| **Cần thêm**: Viết lệnh `#[tauri::command] async fn workspace_delete(...)` hoặc sử dụng `workspace_update` gắn cờ `status: "archived"`. |

---

### 15.3. Phân Hệ Thành Viên & Phân Quyền (Member RBAC)

| Màn Hình / Thao Tác UI | Trạng Thái API | Tên Lệnh Tauri / Backend Method | Vị Trí Code Hiện Tại | Hướng Dẫn Kỹ Thuật / Điều Chỉnh Cần Làm |
|---|:---:|---|---|---|
| **Lấy danh sách Thành viên Workspace** | 🟢 ĐÃ CÓ | `member_list` | `lib.rs:108`, `bridge.ts:167` | Nhận `{ workspaceId }`. Trả về danh sách `Member`. Gọi ngay. |
| **Lấy chi tiết 1 thành viên** | 🟢 ĐÃ CÓ | `member_get` | `lib.rs:109`, `bridge.ts:168` | Nhận `{ workspaceId, memberId }`. Gọi ngay. |
| **Mời thành viên (Tạo lời mời)** | 🟢 ĐÃ CÓ | `member_invite` | `lib.rs:110`, `bridge.ts:169` | Nhận `{ workspaceId, member }`. Trả về `Member` kèm `inviteExpiresAt`. |
| **Chấp nhận lời mời (Activate)** | 🟢 ĐÃ CÓ | `member_activate` | `lib.rs:111`, `bridge.ts:170` | Nhận `{ workspaceId, memberId }`. Gọi ngay. |
| **Đình chỉ / Tạm khóa thành viên** | 🟢 ĐÃ CÓ | `member_suspend` | `lib.rs:112`, `bridge.ts:171` | Nhận `{ workspaceId, memberId }`. Chuyển status sang `suspended`. Gọi ngay. |
| **Cập nhật vai trò (Role Update)** | 🟢 ĐÃ CÓ | `member_role_update` | `lib.rs:113`, `bridge.ts:172` | Nhận `{ workspaceId, memberId, role: "owner"|"admin"|"operator"|"member" }`. Gọi ngay. |
| **Lấy danh sách quyền chi tiết (Grants)**| 🟢 ĐÃ CÓ | `grant_list` | `lib.rs:151`, `bridge.ts:205` | Nhận `{ workspaceId, memberId }`. Trả về danh sách quyền trên từng resource. |
| **Cấp quyền mới (Create Grant)** | 🟢 ĐÃ CÓ | `grant_create` | `lib.rs:153`, `bridge.ts:210` | Nhận `{ workspaceId, resourceId, memberId, action, effect }`. Gọi ngay. |
| **Thu hồi quyền (Revoke Grant)** | 🟢 ĐÃ CÓ | `grant_revoke` | `lib.rs:154`, `bridge.ts:211` | Nhận `{ grantId }`. Gọi ngay. |

---

### 15.4. Phân Hệ Quản Lý Nhóm & Profile (Groups & Profiles)

| Màn Hình / Thao Tác UI | Trạng Thái API | Tên Lệnh Tauri / Backend Method | Vị Trí Code Hiện Tại | Hướng Dẫn Kỹ Thuật / Điều Chỉnh Cần Làm |
|---|:---:|---|---|---|
| **Lấy danh sách nhóm kèm số lượng Profile**| 🟢 ĐÃ CÓ | `get_groups_with_profile_counts`| `lib.rs:165`, `legacy_compat.rs:357`| Trả về `GroupWithCount[]` (`{ id, name, count }`). Gọi ngay. |
| **Tạo nhóm mới (Kèm màu sắc & proxy)** | 🟡 CẦN SỬA | `create_profile_group` | `legacy_compat.rs:392` | Hiện chỉ nhận `{ name: String }`. **Cần sửa**: Mở rộng nhận thêm `appearance: { color, icon }` và `defaultProxyId`. Schema OpenAPI đã hỗ trợ `appearance: object` (dòng 1248). |
| **Cập nhật nhóm (Sửa tên, màu sắc)** | 🟡 CẦN SỬA | `update_profile_group` | `legacy_compat.rs:417` | Hiện chỉ nhận `{ id, name }`. **Cần sửa**: Mở rộng nhận `appearance` và `defaultProxyId`. |
| **Xóa nhóm Profile** | 🟢 ĐÃ CÓ | `delete_profile_group` | `lib.rs:169`, `legacy_compat.rs`| Nhận `{ groupId }`. Xóa nhóm an toàn, giữ nguyên profile. Gọi ngay. |
| **Gán hàng loạt Profile vào nhóm** | 🟢 ĐÃ CÓ | `assign_profiles_to_group` | `lib.rs:170`, `legacy_compat.rs`| Nhận `{ groupId, profileIds }`. Gọi ngay. |
| **Thay thế nhóm của Profile** | 🟢 ĐÃ CÓ | `profile_groups_replace` | `lib.rs:114`, `bridge.ts:173` | Nhận `{ workspaceId, profileId, expectedRevision, groupIds }`. Gọi ngay. |
| **Chạy tất cả Profile trong nhóm (Run All)**| 🟢 SẴN SÀNG | `launch_browser_profile` | `lib.rs:181` | Frontend gọi lặp qua `profileIds` của nhóm với khoảng trễ `throttle: 800ms`. Không cần API mới. |
| **Dừng tất cả Profile trong nhóm (Stop All)**| 🟢 SẴN SÀNG | `kill_browser_profile` | `lib.rs:182` | Frontend gọi `Promise.all(profileIds.map(id => kill_browser_profile(id)))`. Không cần API mới. |

---

### 15.5. Phân Hệ Cơ Chế Mời Không Dùng Email (No-SMTP Invitation Suite)

| Màn Hình / Thao Tác UI | Trạng Thái API | Tên Lệnh Tauri / Backend Method | Vị Trí Code Hiện Tại | Hướng Dẫn Kỹ Thuật / Điều Chỉnh Cần Làm |
|---|:---:|---|---|---|
| **Sinh Mã Mời (Invite Code) & Deep Link** | 🟢 ĐÃ CÓ | `member_invite` | `lib.rs:110` | Trả về `Member` với `id` làm mã mời và `inviteExpiresAt`. Client format thành `buglogin://invite?code={id}`. |
| **Hộp thư lời mời nhận được trong app** | 🔴 CHƯA CÓ | `get_my_pending_invites` | *Chưa đăng ký trong lib.rs* | **Cần thêm**: Viết lệnh `#[tauri::command] async fn get_my_pending_invites()` để lọc các record `Member` có `status == "invited"` thuộc về email người dùng hiện tại. |
| **Nhập mã mời thủ công để tham gia** | 🟡 CẦN ADAPT | `member_activate` | `lib.rs:111` | Lệnh `member_activate` hiện yêu cầu `{ workspaceId, memberId }`. Cần viết thêm 1 hàm bọc `redeem_invite_code(code)` để phân giải `code` thành `workspaceId` rồi kích hoạt. |

---

### 15.6. Phân Hệ Kho Proxy & Đo Tốc Độ (Proxy Fleet)

| Màn Hình / Thao Tác UI | Trạng Thái API | Tên Lệnh Tauri / Backend Method | Vị Trí Code Hiện Tại | Hướng Dẫn Kỹ Thuật / Điều Chỉnh Cần Làm |
|---|:---:|---|---|---|
| **Lấy danh sách Proxy đã lưu** | 🟢 ĐÃ CÓ | `get_stored_proxies` / `proxy_list` | `lib.rs:146, 157` | Trả về danh sách proxy kèm cấu hình port, protocol, credentials. Gọi ngay. |
| **Thêm Proxy mới (Dán hoặc nhập form)** | 🟢 ĐÃ CÓ | `create_stored_proxy` / `proxy_create`| `lib.rs:148, 160` | Gọi ngay. |
| **Xóa Proxy** | 🟢 ĐÃ CÓ | `delete_stored_proxy` / `proxy_delete`| `lib.rs:150, 162` | Gọi ngay. |
| **Kiểm tra Ping thực tế từng Proxy** | 🟢 ĐÃ CÓ | `check_stored_proxy` | `lib.rs:164` | Nhận `{ proxyId }`. Trả về latency ms thực tế hoặc timeout. Gọi ngay. |
| **Kiểm tra Ping hàng loạt (Batch Ping)** | 🟢 SẴN SÀNG | `check_stored_proxy` | `lib.rs:164` | Frontend gọi song song `Promise.allSettled(proxyIds.map(id => check_stored_proxy(id)))`. Hoạt động ngay. |
| **Phân tích danh sách Proxy dạng Text (Parse)**| 🟢 ĐÃ CÓ | `parse_txt_proxies` / `proxy_parse` | `lib.rs:143, 156` | Nhận chuỗi text dán, tự động tách `Host:Port:User:Pass` và kiểm tra hợp lệ. Gọi ngay. |

---

### 15.7. Phân Hệ Nhật Ký Kiểm Toán (Security Audit Log)

| Màn Hình / Thao Tác UI | Trạng Thái API | Tên Lệnh Tauri / Backend Method | Vị Trí Code Hiện Tại | Hướng Dẫn Kỹ Thuật / Điều Chỉnh Cần Làm |
|---|:---:|---|---|---|
| **Lắng nghe luồng sự kiện bảo mật trực tiếp**| 🟢 ĐÃ CÓ | `events_stream` | `lib.rs:83` | Mở kênh sự kiện thời gian thực (Server-Sent Events) qua Tauri. |
| **Dừng luồng sự kiện** | 🟢 ĐÃ CÓ | `events_stream_stop` | `lib.rs:84` | Đóng kênh stream khi dialog tắt. Gọi ngay. |
| **Lấy danh sách lịch sử Audit có phân trang**| 🔴 CHƯA CÓ TRONG LIB.RS| `audit_events_list` | `crates/api/src/commands/registry.rs:280-281`| Schema `AuditEventsListInput` và `AuditEventPage` đã có sẵn trong Rust core API. **Cần thêm**: Đăng ký `#[tauri::command] async fn audit_events_list(...)` vào `lib.rs`. |

---

### 15.8. TỔNG KẾT & KẾ HOẠCH TRIỂN KHAI THEO ĐỘ SẴN SÀNG

1. **85% tính năng ĐÃ CÓ API 100% HOẶC SẴN SÀNG GỌI NGAY**:
   - Toàn bộ luồng Đăng nhập, Đăng xuất, Lấy tài khoản.
   - Toàn bộ luồng Quản lý Tổ Chức (List, Create, Update).
   - Toàn bộ luồng Quản lý Workspace (List, Create, Update, Select Active, Switcher).
   - Toàn bộ luồng Thành viên & Phân Quyền (List, Invite, Activate, Suspend, Role Update, Grant List, Grant Create, Grant Revoke).
   - Toàn bộ luồng Nhóm cơ bản (List with Counts, Delete, Assign Profiles, Run All, Stop All).
   - Toàn bộ luồng Quản lý Kho Proxy & Đo Ping thực tế (Single Ping, Batch Ping, Parse Text).
   - ➜ **Có thể bắt tay vào làm UI ngay lập tức mà không sợ lỗi backend!**

2. **Chỉ 15% cần tinh chỉnh nhỏ (Sẽ bổ sung trực tiếp khi làm tới màn hình tương ứng)**:
   - Thêm trường `appearance: { color, icon }` vào `create_profile_group` và `update_profile_group` trong `legacy_compat.rs` (OpenAPI schema đã có sẵn).
   - Viết lệnh `get_my_pending_invites` và hàm bọc `redeem_invite_code` cho cơ chế mời không email.
   - Viết lệnh `account_password_change` và đăng ký `audit_events_list` vào `lib.rs`.


---

## 16. PHÂN QUYỀN TRUY CẬP THEO NHÓM (GROUP-SCOPED SHARING) & DI CHUYỂN PROFILE (MOVE PROFILE)

> **Mã năng lực hợp đồng**: `CAP-PROF-003`, `CAP-SHARE-001`, `docs/superpowers/specs/2026-10-14-group-scoped-sharing-design.md`  
> **Backend Rust Authority**: `group_share_list`, `group_share_create`, `group_share_revoke`, `assign_profiles_to_group`, `profile_groups_replace`.

---

### 16.1. Nguyên Lý Vận Hành Của Phân Quyền Theo Nhóm (Dynamic Group ACL)

1. **Phân quyền theo Nhóm (Group Grant)**:
   - Thay vì phải chia sẻ lẻ tẻ từng Profile cho nhân viên, Owner/Admin chỉ cần gán quyền của Member vào **Nhóm (Group)**.
   - Lệnh Rust `group_share_create` gắn: `(workspaceId, groupId, memberId, action: "execute"|"read"|"update", effect: "allow")`.
   - **Tính chất Động (Dynamic Cascade)**:
     - Khi một Profile được **Chuyển (Move)** vào Nhóm A ➜ Tất cả nhân viên có quyền trên Nhóm A **tự động nhìn thấy và mở được** Profile đó ngay lập tức mà không cần phân quyền lại!
     - Khi Profile được **Chuyển ra khỏi** Nhóm A ➜ Nhân viên của Nhóm A lập tức **mất quyền truy cập** Profile đó.
2. **Quy Tắc Hiển Thị Tab "Tất Cả Profile" (All Profiles)**:
   - Profile luôn thuộc về Không Gian Làm Việc (Workspace) và hiển thị ở danh sách "Tất Cả Profile".
   - Nhân viên chỉ nhìn thấy các Profile thuộc các Nhóm mà mình được cấp quyền truy cập (hoặc được chia sẻ trực tiếp).
   - Admin và Owner nhìn thấy toàn bộ Profile trong Workspace.

---

### 16.2. ASCII Wireframe: Modal Phân Quyền Truy Cập Nhóm (`GroupSharingModal`)
*Mở từ nút `[ 👥 Phân Quyền ]` trên từng dòng của Bảng Quản Lý Nhóm.*

```text
+----------------------------------------------------------------------+
| 👥 Phân Quyền Truy Cập Nhóm: [ 🔵 Facebook BM Ads Q4 ]       [ ✕ ]   |
| ──────────────────────────────────────────────────────────────────── |
| Nhóm hiện có 120 Profiles. Thành viên được cấp quyền sẽ tự động có   |
| quyền mở/chạy toàn bộ Profile thuộc nhóm này.                        |
|                                                                      |
| Tìm kiếm nhân sự: [ 🔍 Nhập tên hoặc email...                      ] |
|                                                                      |
| DANH SÁCH THÀNH VIÊN TRONG WORKSPACE ĐƯỢC PHÉP TRUY CẬP:             |
| +------------------------------------------------------------------+ |
| | [x]| Thành Viên & Email       | Vai Trò Org | Quyền Trên Nhóm Này | |
| |----+--------------------------+-------------+---------------------| |
| | [x]| [Avatar] Le Van C        | ⚙️ Operator | [ Chỉ Mở/Chạy (▶)▼] | |
| |    |   staff01@alphamedia.vn  |             |                     | |
| | [x]| [Avatar] Pham Thi D      | ⚙️ Operator | [ Chỉ Mở/Chạy (▶)▼] | |
| |    |   staff02@alphamedia.vn  |             |                     | |
| | [x]| [Avatar] Hoang E         | 👥 Manager  | [ Toàn Quyền Nhóm ▼]| |
| |    |   staff03@alphamedia.vn  |             | (Read, Update, Run) | |
| | [ ]| [Avatar] Tran Van F      | ⚙️ Operator | -- Chưa cấp quyền --| |
| |    |   staff04@alphamedia.vn  |             |                     | |
| +------------------------------------------------------------------+ |
|                                                                      |
| [!] Mẹo: Khi bạn chuyển profile mới vào nhóm này, các thành viên     |
|     được tick chọn ở trên sẽ tự động thấy và chạy được profile ngay. |
| ──────────────────────────────────────────────────────────────────── |
| [ Hủy ]                                      [ 💾 Lưu Phân Quyền ]   |
+----------------------------------------------------------------------+
```

---

### 16.3. ASCII Wireframe: Di Chuyển Profile Sang Nhóm Khác (`GroupAssignmentDialog`)
*Hỗ trợ di chuyển 1 Profile (từ menu `⋯` của dòng) hoặc Hàng loạt Profile (từ thanh Bulk Action Bar).*

```text
+----------------------------------------------------------------------+
| 📁 Chuyển Profile Sang Nhóm Khác                             [ ✕ ]   |
| ──────────────────────────────────────────────────────────────────── |
| Đang chọn: 5 Profiles (FB_Ads_01, FB_Ads_02, ...)                   |
|                                                                      |
| Chọn Nhóm Đích Để Chuyển Đến:                                        |
| +--------------------------------------------------------------+---+ |
| | [ 🔵 Facebook BM Ads Q4 (120 profiles)                     ] | ▼ | |
| +--------------------------------------------------------------+---+ |
|                                                                      |
| DANH SÁCH CÁC NHÓM TRONG WORKSPACE:                                  |
| • (•) 🔵 Facebook BM Ads Q4      (120 profiles)  [Nhóm hiện tại]     |
| • ( ) 🟢 TikTok Farm VN          (85 profiles)                       |
| • ( ) 🟣 Crypto Whitelist        (40 profiles)                       |
| • ( ) ⚪ Chưa phân nhóm (Bỏ nhóm) (15 profiles)                      |
|                                                                      |
| [ + Tạo Nhóm Mới Ngay Tại Đây... ]                                   |
|                                                                      |
| Tác động phân quyền sau khi chuyển:                                  |
| ➜ 5 Profiles này sẽ tự động được chia sẻ cho 3 nhân sự đang phụ      |
|   trách nhóm "Facebook BM Ads Q4" (Le Van C, Pham Thi D, Hoang E).   |
| ──────────────────────────────────────────────────────────────────── |
| [ Hủy ]                                        [ 🚀 Chuyển Nhóm Ngay ]|
+----------------------------------------------------------------------+
```

---

### 16.4. Đối Soát API Backend Cho Tính Năng Này

| Thao Tác | Lệnh Backend Thực Tế | Tình Trạng API | Chi Tiết |
|---|---|:---:|---|
| **Lấy danh sách người được chia sẻ nhóm** | `group_share_list` | 🟢 ĐÃ CÓ 100% | Lệnh `group_share_list` trong `lib.rs:836` nhận `{ workspaceId, groupId }`. |
| **Cấp quyền truy cập nhóm cho thành viên**| `group_share_create` | 🟢 ĐÃ CÓ 100% | Lệnh `group_share_create` trong `lib.rs:847` nhận `{ workspaceId, groupId, memberId, action, effect }`. |
| **Thu hồi quyền truy cập nhóm** | `group_share_revoke` | 🟢 ĐÃ CÓ 100% | Lệnh `group_share_revoke` trong `lib.rs:864` nhận `{ workspaceId, groupId, grantId, expectedRevision }`. |
| **Di chuyển Profile sang nhóm khác** | `assign_profiles_to_group` | 🟢 ĐÃ CÓ 100% | Lệnh `assign_profiles_to_group` trong `legacy_compat.rs:498` nhận `{ profileIds, groupId }`. Gọi `profile_groups_replace` và phát event `profiles-changed` để bảng tự cập nhật! |
