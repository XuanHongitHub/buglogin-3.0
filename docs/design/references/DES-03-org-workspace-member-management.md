# DES-03 Reference Package: Workspace Resource & Member Management

Status: approved design specification & interaction contract  
Screen: Workspace Management Surfaces (Members, Groups, Proxies, and Audit Trail)  
Authority: [`docs/design/ui-authority.md`](../ui-authority.md), [`DESIGN.md`](../../DESIGN.md), [`DES-02-unified-switcher-action-hub.md`](DES-02-unified-switcher-action-hub.md)  
Last updated: 2026-10-26  

---

## 1. Nguyên Tắc Thiết Kế Cốt Lõi

1. **Inline Compact Metrics (Triệt tiêu KPI cồng kềnh):** Thay thế toàn bộ các card thống kê lớn bằng dải trạng thái siêu mỏng cao `28px (h-7)`, hiển thị thông số dạng chữ nhỏ (`font-mono text-xs`), phân tách bằng dấu chấm giữa `•`.
2. **Quản trị 3 tầng không xung đột (The 3-Tier Access Hierarchy):**
   - **Tầng 1 (Org Directory):** Quản lý định danh nhân sự tập trung của tổ chức.
   - **Tầng 2 (Workspace Members):** Quản lý vai trò (`Admin`, `Lead`, `Operator`, `Member`) và trạng thái (`Active`, `Invited`, `Suspended`) trong từng dự án cụ thể.
   - **Tầng 3 (Group / Resource Shares):** Giới hạn phạm vi thành viên chỉ được nhìn và chạy các Profile thuộc các Group được tích chọn.
3. **Thao tác tại chỗ (In-Context Drawers):** Mọi hành động thêm thành viên, chia sẻ nhóm, thêm proxy đều mở qua Slide-over Drawer bên phải mà không điều hướng rời khỏi bảng.

---

## 2. Dải Trạng Thái Tinh Gọn (Inline Compact Status Ribbon)

```text
+-------------------------------------------------------------------------------------------------------------------------+
| [TITLEBAR]                                                                                                    [-][口][X]|
| [LuBug] BugLogin │ [ 🏢 Alpha Media / Main WS │ [Admin] [v] ] │ [ ➕ THÊM [v] ]                                 [User Ava]|
+-------------------------------------------------------------------------------------------------------------------------+
| [INLINE COMPACT METRICS RIBBON (h-7: Siêu gọn, tinh tế, font-mono xs)]                                                 |
|   12 Members (10 Active · 2 Invited)  •  Profiles: 128/200 (64%)  •  Storage: 1.2/5.0 GB  •  Sync: 🟢 Synced 2m ago     |
+---+---------------------------------------------------------------------------------------------------------------------+
| [SUB-HEADER TABS: ĐIỀU HƯỚNG TẬP TRUNG]                                                                                 |
|   [ 🌐 Profiles (128) ]   [ 📁 Groups (6) ]   [ 👥 Members (12) ]   [ 🛡️ Proxies (24) ]   [ 📜 Audit Log ]   [ ⚙️ Settings ] |
+---+---------------------------------------------------------------------------------------------------------------------+
```

---

## 3. Màn Hình Quản Lý Thành Viên Cấp Workspace (Workspace Members)

```text
+---+---------------------------------------------------------------------------------------------------------------------+
| R | [HEAD TOOLBAR: BỘ LỌC THÀNH VIÊN & NÚT HÀNH ĐỘNG CHÍNH]                                                             |
| A |  [ 🔍 Tìm thành viên theo tên, email... (/) ]  [ 🏷️ Vai trò: Tất cả [v] ]  [ 🟢 Trạng thái: Active [v] ]               |
| I |---------------------------------------------------------------------------------------------------------------------|
| L | [DATA TABLE: DANH SÁCH THÀNH VIÊN TRONG WORKSPACE]                                                                  |
|   |                                                                                                                     |
|   | [ ] │ Thành viên (Identity)   │ Vai trò trong WS │ Trạng thái │ Nhóm được cấp quyền (Groups)   │ Ngày gia nhập│[Action] |
|   |-----+-------------------------+------------------+------------+--------------------------------+--------------+---------|
|   | [ ] │ 👤 Alex Rivers (Bạn)    │ [Owner]          │ [🟢 Active] │ 🌟 Toàn bộ nhóm (Full Access)  │ 12/01/2026   │ [⋯]     |
|   | [ ] │ 👤 Sarah Connor         │ [Admin]          │ [🟢 Active] │ 🌟 Toàn bộ nhóm (Full Access)  │ 15/01/2026   │ [Sửa][⋯]|
|   | [ ] │ 👤 Michael Chang        │ [Lead]           │ [🟢 Active] │ 📁 Ecom 45, 📁 Crypto 32       │ 02/02/2026   │ [Sửa][⋯]|
|   | [ ] │ 👤 Nguyen Van A (CTV)   │ [Member]         │ [🟢 Active] │ 📁 Chỉ nhóm Social 39          │ 20/03/2026   │ [Sửa][⋯]|
|   | [ ] │ ✉️ designer@freelance.vn│ [Member]         │ [🟡 Invited]│ 📁 Chỉ nhóm Media 12           │ Hết hạn 3 ngày│[Hủy][⋯]|
|   | [ ] │ 👤 Tran Van B (Nghỉ việc│ [Operator]       │ [🔴 Suspend]│ ⛔ Đã thu hồi toàn bộ quyền    │ 10/02/2026   │ [Mở] [⋯]|
+---+---------------------------------------------------------------------------------------------------------------------+
```

---

## 4. Màn Hình Quản Lý Nhóm Profiles (Groups & Shares)

```text
+---+---------------------------------------------------------------------------------------------------------------------+
| R | [HEAD TOOLBAR: GROUPS]                                                                                              |
| A |  [ 🔍 Tìm nhóm profile... (/) ]                                                       [ [➕] Tạo Nhóm mới (Ctrl+G) ]|
| I |---------------------------------------------------------------------------------------------------------------------|
| L | [DATA TABLE: DANH SÁCH NHÓM PROFILES]                                                                               |
|   |                                                                                                                     |
|   | [ ] │ Tên Nhóm (Group)      │ Profiles │ Thành viên được cấp quyền truy cập (Shares)    │ Cập nhật   │ [Hành động]  |
|   |-----+-----------------------+----------+------------------------------------------------+------------+--------------|
|   | [ ] │ 🟢 E-Commerce Store   │ 45 prof  │ 👤 Alex (Owner), 👤 Michael (Lead), 👤 CTV An  │ 10m trước  │ [Chia sẻ][⋯] |
|   | [ ] │ 🟣 Crypto Airdrop     │ 32 prof  │ 👤 Alex (Owner), 👤 Sarah Connor (Admin)       │ 1h trước   │ [Chia sẻ][⋯] |
|   | [ ] │ 🔵 TikTok US Farming  │ 20 prof  │ 👤 Sarah Connor (Admin), 👤 Ken (Member)       │ Hôm qua    │ [Chia sẻ][⋯] |
|   | [ ] │ 🟡 Facebook Ads Black │ 18 prof  │ 🔒 Chỉ Admin & Owner                           │ 3 ngày     │ [Chia sẻ][⋯] |
|   | [ ] │ ⚪ Chờ phân loại      │ 13 prof  │ 🌐 Mọi thành viên trong Workspace              │ 1 tuần     │ [Chia sẻ][⋯] |
+---+---------------------------------------------------------------------------------------------------------------------+
```

---

## 5. Màn Hình Danh Bạ Proxy (Proxy Directory)

```text
+---+---------------------------------------------------------------------------------------------------------------------+
| R | [HEAD TOOLBAR: PROXIES]                                                                                             |
| A |  [ 🔍 Tìm theo IP, Quốc gia, Tag... ]  [ Giao thức: Tất cả [v] ]  [ 🟢 Alive [v] ]       [ [➕] Thêm Proxy ] [⚡ Test All]|
| I |---------------------------------------------------------------------------------------------------------------------|
| L | [DATA TABLE: DANH BẠ PROXY]                                                                                         |
|   |                                                                                                                     |
|   | [ ] │ Host : Port           │ Giao thức │ Quốc gia / Vùng         │ Độ trễ  │ Đang gán cho   │ Trạng thái │ [Hành động]  |
|   |-----+-----------------------+-----------+-------------------------+---------+----------------+------------+--------------|
|   | [ ] │ 192.168.1.10:8080     │ Socks5    │ 🇺🇸 United States (LA)   │ 115ms   │ 3 Profiles     │ [🟢 Alive]  │ [Kiểm tra][⋯]|
|   | [ ] │ 103.25.14.88:9999     │ Http      │ 🇻🇳 Vietnam (HN)         │  24ms   │ 8 Profiles     │ [🟢 Alive]  │ [Kiểm tra][⋯]|
|   | [ ] │ 45.12.89.201:1080     │ Socks5    │ 🇸🇬 Singapore            │  65ms   │ 1 Profile      │ [🟢 Alive]  │ [Kiểm tra][⋯]|
|   | [ ] │ 185.220.101.5:8080    │ Https     │ 🇩🇪 Germany (Frankfurt)  │   --    │ 0 Profile      │ [🔴 Dead]   │ [Kiểm tra][⋯]|
+---+---------------------------------------------------------------------------------------------------------------------+
```

---

## 6. Màn Hình Nhật Ký Kiểm Toán (Audit Trail)

```text
+---+---------------------------------------------------------------------------------------------------------------------+
| R | [HEAD TOOLBAR: AUDIT LOGS]                                                                                          |
| A |  [ 🔍 Tìm sự kiện, người thao tác... ]  [ Hành động: Tất cả [v] ]  [ 24 giờ qua [v] ]            [ [⬇️] Xuất báo cáo CSV]|
| I |---------------------------------------------------------------------------------------------------------------------|
| L | [DATA TABLE: NHẬT KÝ THAO TÁC BẤT BIẾN]                                                                             |
|   |                                                                                                                     |
|   | Thời gian (UTC)  │ Người thực hiện          │ Hành động (Action)           │ Đối tượng tác động      │ Kết quả        |
|   |------------------+--------------------------+------------------------------+-------------------------+----------------|
|   | 26/10 08:45:12   │ alex@alpha.com [Admin]   │ 👤 member.role_change        │ Michael Chang ➔ Lead    │ [🟢 Thành công] |
|   | 26/10 08:30:05   │ sarah@alpha.com [Admin]  │ 🌐 profile.launch            │ Shop_Acc_01 (Wayfern)   │ [🟢 Thành công] |
|   | 26/10 08:12:44   │ alex@alpha.com [Admin]   │ 👥 group.share_grant         │ Ecom Store ➔ CTV An     │ [🟢 Thành công] |
|   | 26/10 07:55:00   │ m.chang@aff.io [Lead]    │ 🛡️ proxy.assign              │ Proxy_US_01 ➔ Ads_09    │ [🟢 Thành công] |
|   | 26/10 07:40:19   │ ctv_an@aff.io [Member]   │ 🔑 account.login_attempt     │ Đăng nhập từ IP mới     │ [🟡 Cảnh báo]  |
+---+---------------------------------------------------------------------------------------------------------------------+
```


---

## 7. Drawer Phân Quyền Thành Viên (Edit Member Role & Scope)

```text
+--------------------------------------------------+------------------------------------+
| DANH SÁCH THÀNH VIÊN Ở NỀN (GIỮ NGUYÊN BẢNG)    | [DRAWER: PHÂN QUYỀN THÀNH VIÊN]  X |
|                                                  +------------------------------------+
|                                                  | Nhân sự: Michael Chang             |
|                                                  | Email:   m.chang@aff.io            |
|                                                  | Ngày vào: 02/02/2026               |
|                                                  +------------------------------------+
|                                                  | 1. Vai trò trong Workspace (Role): |
|                                                  | ( ) Owner (Chủ sở hữu)             |
|                                                  | ( ) Admin (Quản trị viên)          |
|                                                  | (o) Lead (Trưởng nhóm phụ trách)   |
|                                                  | ( ) Operator (Vận hành profile)    |
|                                                  | ( ) Member (Chỉ chạy hồ sơ gán)    |
|                                                  |                                    |
|                                                  | 2. Nhóm Profile được phép quản lý: |
|                                                  | [x] 🟢 E-Commerce Store (45 prof)  |
|                                                  | [x] 🟣 Crypto Airdrop (32 prof)    |
|                                                  | [ ] 🔵 TikTok US Farming (20 prof) |
|                                                  | [ ] 🟡 Facebook Ads Black (18 prof)|
|                                                  |                                    |
|                                                  | 3. Trạng thái hoạt động:           |
|                                                  | [🟢 Đang hoạt động (Active)      v]|
|                                                  | (Chọn: Đình chỉ để ngắt quyền tức  |
|                                                  |  thời mà không cần xóa tài khoản)  |
|                                                  +------------------------------------+
|                                                  | [ Hủy bỏ ]          [ Lưu thay đổi]|
+--------------------------------------------------+------------------------------------+
```

---

## 8. Drawer Cấu Hình Hạn Ngạch & Offline Entitlement (Policy)

```text
+--------------------------------------------------+------------------------------------+
| MÀN HÌNH SETTINGS WORKSPACE                      | [DRAWER: CẤU HÌNH POLICY WORKSPACE]X|
|                                                  +------------------------------------+
|                                                  | Cấu hình giới hạn cho:             |
|                                                  | 📂 Main Production WS              |
|                                                  |                                    |
|                                                  | 1. Giới hạn Hồ sơ (Max Profiles):  |
|                                                  | [ 200     ] profiles               |
|                                                  | (Hiện dùng: 128/200 - 64%)         |
|                                                  |                                    |
|                                                  | 2. Giới hạn Thành viên:            |
|                                                  | [ 20      ] members                |
|                                                  | (Hiện dùng: 12/20 - 60%)           |
|                                                  |                                    |
|                                                  | 3. Dung lượng lưu trữ tối đa:      |
|                                                  | [ 10.0    ] GB                     |
|                                                  |                                    |
|                                                  | 4. Quyền chạy Offline (Local):     |
|                                                  | [x] Kích hoạt Offline Entitlement  |
|                                                  | 💡 Cho phép máy trạm mở profile    |
|                                                  |    ngay cả khi mất kết nối mạng.   |
|                                                  +------------------------------------+
|                                                  | [ Khôi phục mặc định ]  [ Áp dụng ]|
+--------------------------------------------------+------------------------------------+
```

---

## 9. Ma Trận Trạng Thái Giao Diện Đặc Biệt (Empty & Forbidden States)

### 9.1. Trạng Thái Workspace Rỗng (Empty State)
```text
+---+---------------------------------------------------------------------------------------------------------------------+
| R | [HEAD TOOLBAR]  [ 🔍 Tìm thành viên... ]                                         [ [➕] Thêm Thành viên vào WS ]    |
| A |---------------------------------------------------------------------------------------------------------------------|
| I |                                                                                                                     |
| L |                                           [ 👥 ]                                                                    |
|   |                              Chưa có thành viên nào trong Workspace này                                             |
|   |                     Hãy mời cộng sự hoặc thêm nhân sự từ Danh bạ Tổ chức để bắt đầu phân việc.                     |
|   |                                                                                                                     |
|   |                                    [ [➕] Thêm Thành viên đầu tiên ]                                                |
|   |                                                                                                                     |
+---+---------------------------------------------------------------------------------------------------------------------+
```

### 9.2. Trạng Thái Bị Đình Chỉ / Từ Chối Quyền (Forbidden State - 403)
```text
+-------------------------------------------------------------------------------------------------------------------------+
| [TITLEBAR: BUGOGIN DESKTOP]                                                                                   [-][口][X]|
+-------------------------------------------------------------------------------------------------------------------------+
|                                                                                                                         |
|                                                      [ ⛔ ]                                                             |
|                                            TRUY CẬP BỊ TỪ CHỐI (403)                                                    |
|                                                                                                                         |
|                       Tài khoản của bạn đang ở trạng thái [Suspended] tại Workspace này.                                |
|                        Toàn bộ quyền khởi chạy Profile và đồng bộ dữ liệu đã tạm thời bị khóa.                         |
|                                                                                                                         |
|                     Lý do: Quyết định quản trị viên (Admin: alex@alpha.com cập nhật lúc 08:45).                         |
|                                                                                                                         |
|                                     [ 🔄 Kiểm tra lại quyền ]    [ 🏢 Đổi Workspace ]                                   |
|                                                                                                                         |
+-------------------------------------------------------------------------------------------------------------------------+
```


---

## 10. Màn Hình Danh Sách Tổ Chức (Organization List Master Table & Drawer)

```text
+-------------------------------------------------------------------------------------------------------------------------+
| [TITLEBAR: APP CONTEXT]                                                                                       [-][口][X]|
| [LuBug] BugLogin │ [ 🏢 Quản lý Tổ chức (Organizations) ] │ [LuBuilding2]                                      [User Ava]|
+-------------------------------------------------------------------------------------------------------------------------+
| [INLINE COMPACT METRICS (h-7)]                                                                                          |
|   12 Organizations (4 Owned · 8 Joined)  •  38 Workspaces  •  1,450 Profiles  •  Storage: 8.4 GB                        |
+---+---------------------------------------------------------------------------------------------------------------------+
| R | [HEAD TOOLBAR: SEARCH & ACTIONS]                                                                                    |
| A |  [ 🔍 Tìm tổ chức theo tên, owner... (/) ]  [ 🏷️ Vai trò: Tất cả [v] ]  [ 🟢 Trạng thái: Active [v] ]               |
| I |---------------------------------------------------------------------------------------------------------------------|
| L | [DATA TABLE: DANH SÁCH TỔ CHỨC]                                                                                     |
|   |                                                                                                                     |
|   | [ ] │ Tên Tổ chức       │ Chủ sở hữu (Owner)   │ Vai trò của bạn │ Workspaces │ Profiles / Quota │ Ngày tạo   │ [Actions] |
|   |-----+-------------------+----------------------+-----------------+------------+------------------+------------+-----------|
|   | [ ] │ 🏢 Alpha Media     │ alex@alpha.com       │ [Org Owner]     │    5 WS    │ 340 / 500  (68%) │ 12/01/2026 │ [Vào] [⋯] |
|   | [ ] │ 🏢 Beta Studio     │ admin@betastudio.vn  │ [Org Admin]     │    3 WS    │ 120 / 200  (60%) │ 20/02/2026 │ [Vào] [⋯] |
|   | [ ] │ 🏢 Global Aff Hub  │ contact@affhub.io    │ [Org Member]    │    2 WS    │  45 / 100  (45%) │ 05/03/2026 │ [Vào] [⋯] |
|   | [ ] │ 🏢 K-Commerce Ops  │ ken@kcom.kr          │ [Org Member]    │    1 WS    │  12 / 50   (24%) │ 18/04/2026 │ [Vào] [⋯] |
|   | [ ] │ 🏢 Sky Ads Agency  │ owner@skyads.com     │ [Org Owner]     │    8 WS    │ 780 / 1000 (78%) │ 24/05/2026 │ [Vào] [⋯] |
+---+---------------------------------------------------------------------------------------------------------------------+
```

### Drawer Tạo Tổ Chức Mới:
```text
+--------------------------------------------------+------------------------------------+
| DANH SÁCH TỔ CHỨC (BẢNG Ở NỀN)                   | [DRAWER: TẠO TỔ CHỨC MỚI]        X |
|                                                  +------------------------------------+
|                                                  | Tên Tổ chức: (*)                   |
|                                                  | [ Alpha Holdings                 ] |
|                                                  |                                    |
|                                                  | Email Chủ sở hữu (Owner Account):  |
|                                                  | [ alex@alpha.com                 ] |
|                                                  |                                    |
|                                                  | Khởi tạo Workspace mặc định:       |
|                                                  | [x] Tạo luôn "Default Workspace"   |
|                                                  |                                    |
|                                                  | Hạn ngạch ban đầu (Quota Policy):  |
|                                                  | - Tối đa Workspaces: [ 10       ]  |
|                                                  | - Tối đa Profiles:   [ 200      ]  |
|                                                  +------------------------------------+
|                                                  | [ Hủy bỏ ]      [ [➕] Tạo Tổ chức ]|
+--------------------------------------------------+------------------------------------+
```

---

## 11. Màn Hình Danh Bạ Workspace Tập Trung (Workspace Directory)

```text
+-------------------------------------------------------------------------------------------------------------------------+
| [TITLEBAR: APP CONTEXT]                                                                                       [-][口][X]|
| [LuBug] BugLogin │ [ 📂 Quản lý Không gian làm việc (Workspaces) ] │ [LuFolderGit2]                            [User Ava]|
+-------------------------------------------------------------------------------------------------------------------------+
| [INLINE COMPACT METRICS (h-7)]                                                                                          |
|   18 Workspaces  •  1,280 Profiles  •  42 Active Members  •  Storage: 6.2 GB                                            |
+---+---------------------------------------------------------------------------------------------------------------------+
| R | [HEAD TOOLBAR: BỘ LỌC TỔ CHỨC & TÌM KIẾM TẬP TRUNG]                                                                 |
| A |  [ 🔍 Tìm Workspace theo tên... (/) ]  [ 🏢 Tổ chức: Tất cả [v] ]  [ 🏷️ Role: Tất cả [v] ]  [ 🟢 Trạng thái [v] ]    |
| I |---------------------------------------------------------------------------------------------------------------------|
| L | [DATA TABLE: DANH SÁCH WORKSPACES]                                                                                  |
|   |                                                                                                                     |
|   | [ ] │ Tên Workspace          │ Thuộc Tổ chức (Parent Org) │ Vai trò của bạn │ Thành viên │ Profiles / Quota  │ [Actions]  |
|   |-----+------------------------+----------------------------+-----------------+------------+-------------------+------------|
|   | [ ] │ 📂 Main Production WS  │ 🏢 Alpha Media (Owner)     │ [Admin]         │ 12 người   │ 128 / 200 (64%)   │ [Vào]  [⋯] |
|   | [ ] │ 📂 E-Commerce Store    │ 🏢 Alpha Media (Owner)     │ [Admin]         │  4 người   │  45 / 100 (45%)   │ [Vào]  [⋯] |
|   | [ ] │ 📂 TikTok Automation   │ 🏢 Alpha Media (Owner)     │ [Lead]          │  2 người   │  30 /  50 (60%)   │ [Vào]  [⋯] |
|   | [ ] │ 📂 Studio Global Dev   │ 🏢 Beta Studio             │ [Member]        │ 18 người   │  80 / 150 (53%)   │ [Vào]  [⋯] |
|   | [ ] │ 👤 Không gian Cá nhân  │ (Không gian cá nhân)       │ [Owner]         │  1 người   │  12 /  20 (60%)   │ [Vào]  [⋯] |
+---+---------------------------------------------------------------------------------------------------------------------+
```

### Drawer Tạo Workspace Mới:
```text
+--------------------------------------------------+------------------------------------+
| DANH SÁCH WORKSPACE (BẢNG Ở NỀN)                 | [DRAWER: TẠO WORKSPACE MỚI]      X |
|                                                  +------------------------------------+
|                                                  | 1. Chọn Tổ chức trực thuộc: (*)    |
|                                                  | [ 🏢 Alpha Media (Org Owner)    v ]|
|                                                  |                                    |
|                                                  | 2. Tên Workspace: (*)              |
|                                                  | [ Facebook Ads Campaign Q4       ] |
|                                                  |                                    |
|                                                  | 3. Hạn ngạch (Quota Policy):       |
|                                                  | - Tối đa Profiles: [ 100        ]  |
|                                                  | - Tối đa Members:  [ 5          ]  |
|                                                  | - [x] Bật quyền chạy Offline       |
|                                                  +------------------------------------+
|                                                  | [ Hủy ]        [ [➕] Tạo Workspace]|
+--------------------------------------------------+------------------------------------+
```

---

## 12. Màn Hình Danh Bạ Nhân Sự Cấp Tổ Chức (Org-level Member Directory)

```text
+-------------------------------------------------------------------------------------------------------------------------+
| [TITLEBAR]                                                                                                    [-][口][X]|
| [LuBug] BugLogin │ [ 🏢 Quản lý Tổ chức: Alpha Media ] │ [ 👥 Danh bạ Thành viên Tổ chức ]                    [User Ava]|
+-------------------------------------------------------------------------------------------------------------------------+
| [INLINE COMPACT METRICS (h-7)]                                                                                          |
|   24 Total Personnel  •  22 Active  •  2 Suspended  •  Coverage: 5 Workspaces                                           |
+---+---------------------------------------------------------------------------------------------------------------------+
| R | [HEAD TOOLBAR]  [ 🔍 Tìm theo tên, email... ]  [ Lọc theo Workspace [v] ]        [ [➕] Thêm nhân sự vào Org ]      |
| A |---------------------------------------------------------------------------------------------------------------------|
| I | [DATA TABLE: DANH BẠ NHÂN SỰ TỔ CHỨC]                                                                               |
| L |                                                                                                                     |
|   | Tên nhân sự / Email        │ Trạng thái Org │ Các Workspace đang tham gia              │ Ngày tạo   │ Hành động     |
|   |----------------------------+----------------+------------------------------------------+------------+---------------|
|   | 👤 Alex Rivers (alex@..)   │ [🟢 Active]    │ [Main WS (Owner)], [TikTok (Admin)]      │ 12/01/2026 │ [Phân bổ] [⋯] |
|   | 👤 Sarah Connor (sarah@..) │ [🟢 Active]    │ [Main WS (Admin)]                        │ 15/01/2026 │ [Phân bổ] [⋯] |
|   | 👤 Michael Chang (m.ch@..) │ [🟢 Active]    │ [Main WS (Lead)], [Ecom Store (Lead)]    │ 02/02/2026 │ [Phân bổ] [⋯] |
|   | 👤 Tran Van B (b.tran@..)  │ [🔴 Suspended] │ ⛔ Bị đình chỉ khỏi TẤT CẢ Workspace     │ 10/02/2026 │ [Kích hoạt][⋯]|
+-------------------------------------------------------------------------------------------------------------------------+
```

---

## 13. Màn Hình Quản Trị Toàn Cục (Platform Admin Global Console)

```text
+-------------------------------------------------------------------------------------------------------------------------+
| [TITLEBAR: PLATFORM ADMIN CONSOLE]                                                                            [-][口][X]|
| [LuBug] BugLogin │ [ 🛡️ PLATFORM ROOT GOVERNANCE (Global Scope) ]                                             [Admin Key]|
+-------------------------------------------------------------------------------------------------------------------------+
| [INLINE COMPACT SYSTEM HEALTH RIBBON (h-7)]                                                                             |
|   PostgreSQL: 🟢 Connected  •  Redis: 🟢 Synced  •  Active Tenancies: 142 Orgs · 680 WS  •  Audit Events: 42,910 (24h)   |
+---+---------------------------------------------------------------------------------------------------------------------+
| [PLATFORM ADMIN TABS]                                                                                                   |
|   [ 🏢 All Organizations ]  [ 📂 All Workspaces ]  [ 👥 Master Accounts ]  [ 📜 Global Audit ]  [ 🛡️ Security / Trust Keys ] |
+---+---------------------------------------------------------------------------------------------------------------------+
| R | [HEAD TOOLBAR: GLOBAL SEARCH & GOVERNANCE]                                                                          |
| A |  [ 🔍 Tìm Org ID, Account ID, Workspace ID, Email... (/) ]  [ Bộ lọc trạng thái [v] ]   [ [⚙️] System Settings ]     |
| I |---------------------------------------------------------------------------------------------------------------------|
| L | [DATA TABLE: TẤT CẢ TỔ CHỨC TOÀN HỆ THỐNG (MULTI-TENANT OVERVIEW)]                                                  |
|   |                                                                                                                     |
|   | Org ID        │ Tên Tổ chức        │ Chủ tài khoản (Master) │ Workspaces │ Profiles │ Storage  │ Trạng thái │ Actions |
|   |---------------+--------------------+------------------------+------------+----------+----------+------------+---------|
|   | org_01j9a1..  │ Alpha Media Group  │ alex@alpha.com         │ 5 WS       │ 340 prof │ 2.4 GB   │ [🟢 Active]│ [Audit] |
|   | org_01j9a2..  │ Beta Game Studio   │ admin@betastudio.vn    │ 3 WS       │ 120 prof │ 850 MB   │ [🟢 Active]│ [Audit] |
|   | org_01j9a3..  │ Black Hat Probe    │ suspect@unknown.net    │ 1 WS       │  15 prof │ 120 MB   │ [🔴 Quarant│ [Khóa!] |
+---+---------------------------------------------------------------------------------------------------------------------+
```
