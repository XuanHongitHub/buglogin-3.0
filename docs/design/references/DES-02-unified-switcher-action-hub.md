# DES-02 Reference Package: Unified Switcher & Context Action Hub

Status: approved design specification & interaction contract  
Screen: Titlebar Context Switcher & Unified Creation Action Hub  
Authority: [`docs/design/ui-authority.md`](../ui-authority.md), [`DESIGN.md`](../../DESIGN.md), [`docs/design/org-workspace-access-resource-model.md`](../org-workspace-access-resource-model.md)  
Last updated: 2026-10-26  

---

## 1. Mục Tiêu và Vấn Đề Giải Quyết

### 1.1. Thực trạng trước khi tối ưu (Current Flaws)
1. **Mất dấu vai trò (Missing Role Context):** Switcher ở thanh Titlebar chỉ hiển thị tên Workspace đơn độc, không phản ánh vai trò của người dùng hiện tại (`Admin`, `Lead` hay `Member`).
2. **Nút "Thêm" bị xé nhỏ và phân tán (Fragmented Actions):**
   - Nút tạo Profile nằm ở toolbar dưới cùng bên phải.
   - Nút tạo Group nằm ở thanh tab ngang phía trên.
   - Nút tạo Workspace nằm ẩn sâu trong popover Switcher.
   - Nút Mời thành viên / Cấp tài khoản biến mất khỏi màn hình làm việc chính, buộc người dùng phải chuyển trang làm mất hoàn toàn phiên làm việc.
3. **Mâu thuẫn phân quyền (Role Violation):** Các vai trò không có quyền (như Member thường) vẫn thấy các nút tạo tài nguyên, bấm vào mới bị lỗi ngầm hoặc trả về lỗi 403 Forbidden khó hiểu.

### 1.2. Mục tiêu thiết kế mới (Target Goals)
1. **Thiết lập "Cặp bài trùng Đầu não" (Context + Action Hub):** Đặt cạnh nhau trên Titlebar:
   - **Switcher:** Định vị *Tôi đang ở đâu? Tôi là ai trong không gian này?*
   - **Nút THÊM (+):** Trả lời câu hỏi *Tôi có quyền tạo những gì tại đây?*
2. **Thích ứng động theo Role (Role-Gated Actions):** Tự động lọc và hiển thị chính xác các hành động mà vai trò hiện tại được phép thực hiện.
3. **Thao tác tại chỗ (In-Context Drawer):** Mọi hành vi tạo lập (Profile, Group, Mời thành viên, Cấp tài khoản) đều kích hoạt qua Slide-over Drawer bên phải mà không điều hướng rời khỏi bảng dữ liệu.

---

## 2. Sơ Đồ Kiến Trúc Giao Diện (ASCII Master Layout)

### 2.1. Thanh Titlebar Tối Ưu

```text
+-------------------------------------------------------------------------------------------------------------------+
| [UNIFIED CONTEXT TITLEBAR]                                                                              [-][口][X]|
|                                                                                                                   |
| [LuBug] BugLogin │ [ [LuBuilding2] Alpha Agency / Main WS │ [Admin] [v] ] │ [ [LuPlus] THÊM [v] ]        [LuUser] |
|                             (A) Neo ngữ cảnh (Org / WS + Role)                   (B) Unified Action Hub           |
+-------------------------------------------------------------------------------------------------------------------+
| [SUB-HEADER: GROUPS & VIEWS]                                                                                      |
|   All 128  │  Starred 12  │  E-commerce 45  │  Crypto 32  │  [ [LuFolderPlus] ] (Icon tạo nhanh Group: N-G)       |
+---+---------------------------------------------------------------------------------------------------------------+
| R | [HEAD TABLE TOOLBAR]                                                                                          |
| A |  [ Search profiles... (/) ]  [ Filter (2) [v] ]  [ Sort [v] ]  [ Columns [v] ]       Quota: 128/200 Profiles  |
| I |  (Toolbar tập trung 100% cho TÌM KIẾM & BỘ LỌC, không nhồi nhét nút tạo profile ở góc phải nữa)               |
+---+---------------------------------------------------------------------------------------------------------------+
```

---

## 3. Đặc Tả Popover Switcher (Bộ Neo Ngữ Cảnh)

Khi người dùng click vào `[ [LuBuilding2] Alpha Agency / Main WS │ [Admin] [v] ]`:

```text
┌─────────────────────────────────────────────────────────────────────────────────┐
│ [LuSearch] Tìm nhanh Tổ chức hoặc Workspace... (Ctrl+W)                         │
├─────────────────────────────────────────────────────────────────────────────────┤
│ TỔ CHỨC CỦA TÔI (SỞ HỮU / QUẢN TRỊ)                                             │
│ ┌─ [LuBuilding] Alpha Agency (Org Owner)                                        │
│ │   ● [LuFolderGit2] Main Workspace             [Admin]  128/200 Profiles  [✓]  │
│ │     [LuFolderGit2] E-Commerce Store           [Admin]   45/100 Profiles       │
│ │     [LuFolderGit2] TikTok Automation          [Lead]    30/50  Profiles       │
│ └─ [LuPlus] Tạo Workspace mới trong Alpha Agency...                             │
├─────────────────────────────────────────────────────────────────────────────────┤
│ TỔ CHỨC ĐƯỢC MỜI THAM GIA (LIÊN KẾT NGOÀI)                                      │
│ ┌─ [LuBuilding] Global Affiliate Hub                                            │
│ │     [LuFolderGit2] Reviewer Pod 02            [Member]  Chỉ đọc (Read-only)   │
├─────────────────────────────────────────────────────────────────────────────────┤
│ KHÔNG GIAN CÁ NHÂN                                                              │
│     [LuUser] My Personal Workspace              [Owner]   12/20  Profiles       │
├─────────────────────────────────────────────────────────────────────────────────┤
│ [LuLayers] Quản lý Tổ chức & Hạn ngạch (Quota Settings)                         │
└─────────────────────────────────────────────────────────────────────────────────┘
```

### Quy tắc hiển thị Switcher:
- **Phân cấp 2 tầng rõ ràng:** Tổ chức (Org) ➔ Không gian làm việc (Workspaces).
- **Role Badge:** Gắn cố định bên cạnh từng Workspace (`[Owner]`, `[Admin]`, `[Lead]`, `[Member]`).
- **Chỉ báo Quota:** Hiển thị tức thời tỷ lệ sử dụng (`128/200 Profiles`).

---

## 4. Đặc Tả Nút THÊM (+) Tự Động Thích Ứng Theo Role

### 4.1. Giao diện khi ở vai trò [ADMIN / OWNER]
Mở đầy đủ cả 2 phân hệ Tài nguyên và Nhân sự:

```text
BẤM [ ➕ THÊM [v] ] (Ngữ cảnh: Main WS - Quyền: Admin)
┌────────────────────────────────────────────────────────────┐
│ 📦 TÀI NGUYÊN HỆ THỐNG                                     │
│   ├── [LuGlobe] Tạo Profile trình duyệt mới       (Ctrl+N) │
│   ├── [LuFolderPlus] Tạo Nhóm Profile (Group)     (Ctrl+G) │
│   └── [LuShieldCheck] Thêm Proxy vào danh bạ               │
├────────────────────────────────────────────────────────────┤
│ 👥 NHÂN SỰ & QUYỀN TRUY CẬP                                │
│   ├── [LuMailPlus] Mời thành viên mới vào WS      (Invite) │
│   └── [LuKeyRound] Cấp tài khoản trực tiếp        (Pass)   │
└────────────────────────────────────────────────────────────┘
```

### 4.2. Giao diện khi ở vai trò [LEAD / OPERATOR]
Ẩn hoàn toàn tính năng nhân sự, chỉ giữ lại tài nguyên trong phạm vi cấp quyền:

```text
BẤM [ ➕ THÊM [v] ] (Ngữ cảnh: TikTok Automation - Quyền: Lead)
┌────────────────────────────────────────────────────────────┐
│ 📦 TÀI NGUYÊN (Trong phạm vi được giao)                    │
│   ├── [LuGlobe] Tạo Profile trình duyệt mới       (Ctrl+N) │
│   └── [LuFolderPlus] Tạo Nhóm con                          │
├────────────────────────────────────────────────────────────┤
│ 👥 NHÂN SỰ                                                 │
│   └── 🔒 Bạn không có quyền cấp tài khoản hoặc mời nhân sự │
└────────────────────────────────────────────────────────────┘
```

### 4.3. Giao diện khi ở vai trò [MEMBER]
Chuyển về trạng thái chỉ đọc (Read-only) hoặc chỉ tạo tài nguyên cá nhân nếu được phân bổ Quota riêng.

---

## 5. Quy Chuẩn Thao Tác Tại Chỗ (In-Context Slide Drawer)

Tuyệt đối không chuyển trang khi thêm tài nguyên hoặc nhân sự:

```text
+----------------------------------------------------+------------------------------------+
| CỬA SỔ CHÍNH GIỮ NGUYÊN BẢNG LÀM VIỆC              | [DRAWER: MỜI / CẤP THÀNH VIÊN]   X |
|                                                    +------------------------------------+
| 🏢 Alpha Media / Main WS [Admin]                   | Ngữ cảnh: Main Workspace           |
|                                                    |                                    |
| [ Bảng 128 Profiles vẫn hiển thị nguyên vẹn ở nền, ]| Hình thức thêm:                    |
| [ không bị load lại trang, không mất bộ lọc. ]     | (o) Gửi link mời qua Email         |
|                                                    | ( ) Cấp tài khoản + Mật khẩu ngay  |
|                                                    |                                    |
|                                                    | Email người nhận:                  |
|                                                    | [ developer@team.com             ] |
|                                                    |                                    |
|                                                    | Gán vai trò (Role):                |
|                                                    | [ Operator                     v ] |
|                                                    |                                    |
|                                                    | Nhóm được truy cập ban đầu:        |
|                                                    | [x] E-commerce 45   [ ] Crypto 32  |
|                                                    |                                    |
|                                                    | [ Hủy ]             [ Xác nhận ]   |
+----------------------------------------------------+------------------------------------+
```

---

## 6. Máy Trạng Thái Khi Đổi Switcher (State Machine)

1. **Hủy Request cũ:** Gửi tín hiệu AbortSignal hủy các truy vấn đang dở dang của Workspace trước.
2. **Reset Con trỏ (Cursor):** Xóa sạch bộ nhớ phân trang để tránh tải dữ liệu chéo giữa 2 Workspace.
3. **Cập nhật Ngữ cảnh tức thì:**
   - Header API: `x-buglogin-workspace-id = <workspace_id_mới>`.
   - Cập nhật Role Badge trên Titlebar.
   - Đánh giá lại cây quyền hạn của nút `[➕ THÊM]`.
4. **Hiển thị Skeleton Loader chuẩn xác:** Giữ nguyên khung cấu trúc bảng, hiển thị skeleton đúng kích thước, triệt tiêu hiện tượng nháy trắng màn hình (Zero Layout Shift).
