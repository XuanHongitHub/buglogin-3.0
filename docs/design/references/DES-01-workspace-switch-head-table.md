# DES-01 Reference Package: Workspace Switcher & Unified Head Table

Status: draft reference package  
Screen: DES-01 Profiles Home (Titlebar, Workspace Switch, and Table Header)  
Authority: `docs/design/ui-authority.md`, `DESIGN.md`, `docs/design/information-architecture.md`, pinned Donut upstream source (`.tmp/donutbrowser-upstream`).  
Last updated: 2026-10-19  

---

## 1. Mục tiêu và Nguyên tắc Thiết kế

### 1.1. Vấn đề giải quyết
1. **Loại bỏ phân tán giữa Search và Filter**: Gom toàn bộ tìm kiếm từ khóa, bộ lọc phân diện (faceted filters), và danh sách chip lọc đang kích hoạt vào cùng một cụm điều khiển đầu bảng (`Head Table Toolbar`).
2. **Nâng cấp cơ chế Sort (Khắc phục "Sort kém")**: Thay thế cơ chế click đơn lẻ một cột bằng hệ thống **Multi-level Sort** (sắp xếp đa tầng có thứ tự ưu tiên), hỗ trợ Popover quản lý tiêu chí và chỉ báo trực quan trên header cột (`↑1`, `↓2`).
3. **Chuẩn hóa Workspace Switcher**: Đặt đúng vị trí trong hệ thống phân cấp (Titlebar bên trái), tách biệt hoàn toàn khỏi danh sách Group tabs để không gây tràn hay vỡ layout khi số lượng nhóm tăng.
4. **Giảm thiểu rác UI (UI Noise Reduction)**:
   - Loại bỏ các chuỗi trạng thái kỹ thuật không cần thiết như `(Local-first)`, `(Synced)`.
   - Thay thế toàn bộ emoji bằng **Lucide Icons** chuẩn (`react-icons/lu`) đồng bộ với Donut upstream (`LuBuilding2`, `LuSearch`, `LuListFilter`, `LuArrowUpDown`, `LuPlus`,...).

---

## 2. Ánh xạ Component Upstream (Donut Baseline Mapping)

| Khối giao diện | Component Upstream (`.tmp/donutbrowser-upstream`) | Ghi chú tích hợp BugLogin |
| :--- | :--- | :--- |
| **Titlebar & Workspace** | `src/components/home-header.tsx` | Chứa logo, Workspace Popover Trigger, System status icon, Window controls |
| **Group Strip** | `src/components/ui/scroll-strip.tsx` | Thanh trượt ngang chứa các preset view / nhóm profile (`All`, `Group A`,...) |
| **Filter Controls** | `src/components/table-controls.tsx` (`TableFilterControls`) | Search input, nút `LuListFilter`, Popover facets, Active filter chips |
| **Sort & View Controls**| `src/components/table-controls.tsx` (`TableViewControls`) | Multi-sort popover trigger, Column visibility selector |
| **Data Table Header** | `src/components/ui/table.tsx`, `DATA_TABLE_CLASSES` | Sticky header `h-8`, Sort direction badges, Column resizers |
| **Floating Action Bar** | `src/components/data-table-action-bar.tsx` | Thanh hành động nổi khi có profile được chọn (`DataTableActionBar`) |

---

## 3. Bản vẽ ASCII Chi tiết: Tổng thể Giao diện (Master Layout)

```text
+-------------------------------------------------------------------------------------------------------------------------+
| [WINDOW TITLEBAR: BRAND + WORKSPACE SWITCHER + WINDOW CONTROLS]                                               [-][口][X]|
|                                                                                                                         |
| [LuBug] BugLogin │ [ [LuBuilding2] Main Workspace  [LuChevronDown] ] │ [LuCloudCheck]                      [LuUser]     |
+-------------------------------------------------------------------------------------------------------------------------+
| [SUB-HEADER: PROFILE GROUPS & SAVED VIEWS]                                                                              |
|                                                                                                                         |
|   All 128  │  [LuStar] Starred 12  │  E-commerce 45  │  Crypto 32  │  Social 39  │  [LuPlus] New Group                  |
+---+---------------------------------------------------------------------------------------------------------------------+
| R | [HEAD TABLE: UNIFIED TOOLBAR]                                                                                       |
| A |                                                                                                                     |
| I |  [LuSearch] [ Search profiles... (/)  ] [LuX]  [ [LuListFilter] Filter (2) [LuChevronDown] ]                       |
| L |                                                                                                                     |
|   |  Active: [ Status: Active [LuX] ] [ Proxy: Alive [LuX] ] [ Tag: Shopee [LuX] ]  Clear all     Showing 24 of 128     |
| N |                                                                                                                     |
| A |  -----------------------------------------------------------------------------------------------------------------  |
| V |  [ [LuArrowUpDown] Sort: Status, Name [2] [LuChevronDown] ]  [ [LuColumns3] Columns ]  │  [ [LuPlus] New Profile ]  |
+---+---------------------------------------------------------------------------------------------------------------------+
|   | [DATA TABLE HEADER (Sticky)]                                                                                        |
|   |                                                                                                                     |
|   | [ ] │ Profile Name      ↑2 │ Status    ↓1 │ Proxy / VPN     ↕ │ Tags        │ Browser / OS │ Note       │ [LuMore]  |
|   |-----+----------------------+--------------+-------------------+-------------+--------------+------------+-----------|
|   | [ ] │ Shop_Acc_01          │ Running      │ 192.168.1.10 (US) │ Ecom, US    │ Cr 132 (Win) │ Primary    │ [LuPlay] ⋯|
|   | [ ] │ Backup_FB_02         │ Stopped      │ Direct            │ Social      │ Fx 130 (Mac) │ Sim test   │ [LuPlay] ⋯|
|   | [ ] │ Ads_Tiktok_09        │ Error        │ 10.0.0.5 (SG)     │ Media       │ Cr 132 (Win) │ Auth fail  │ [LuPlay] ⋯|
+---+---------------------------------------------------------------------------------------------------------------------+
|   | ◀  Page 1 of 6  ▶   [ 50 / page [LuChevronDown] ]                                                                   |
+---+---------------------------------------------------------------------------------------------------------------------+
```

---

## 4. Chi tiết từng Khối Giao diện & Popover

### 4.1. Workspace Switcher Popover (`[LuBuilding2] Main Workspace [LuChevronDown]`)
*Hiển thị phân cấp rõ ràng giữa Workspace Cá nhân (Personal) và Tổ chức (Organizations), kèm tìm kiếm nhanh.*

```text
┌──────────────────────────────────────────────┐
│ [LuSearch] Find workspace... (Ctrl+W)        │
├──────────────────────────────────────────────┤
│ PERSONAL                                     │
│   ✓ [LuUser] Personal Workspace       Owner  │
├──────────────────────────────────────────────┤
│ ORGANIZATIONS                                │
│     [LuBuilding2] Alpha Agency         Admin  │
│     [LuBuilding2] Beta Studio         Member │
├──────────────────────────────────────────────┤
│ [LuPlus] Create Workspace                    │
│ [LuSettings] Manage Workspaces               │
└──────────────────────────────────────────────┘
```

---

### 4.2. Unified Search & Filter Cluster
*Search box và Filter trigger đặt cạnh nhau, chia sẻ cùng ngữ cảnh dữ liệu.*

#### A. Thanh công cụ và Active Chips
```text
┌──────────────────────────────────────┐  ┌───────────────────────────────┐
│ [LuSearch] Search profiles... (/) [x]│  │ [LuListFilter] Filter (2) [v] │
└──────────────────────────────────────┘  └───────────────────────────────┘

Active Chips:
[ Status: Running [LuX] ]  [ Proxy: Active [LuX] ]  [ Tag: Ecom [LuX] ]  [Clear all]
```

#### B. Filter Facets Popover
```text
┌─────────────────────────────────────────────────────────────────┐
│ [LuSearch] Filter criteria...                                   │
├─────────────────────────────────────────────────────────────────┤
│ RUNTIME STATUS                                                  │
│   [x] Running                                              (14) │
│   [ ] Stopped                                             (102) │
│   [ ] Error / Stalled                                      (12) │
├─────────────────────────────────────────────────────────────────┤
│ PROXY STATUS                                                    │
│   [x] Alive                                                (98) │
│   [ ] No Proxy (Direct)                                    (15) │
│   [ ] Dead / Timeout                                       (15) │
├─────────────────────────────────────────────────────────────────┤
│ PLATFORM & BROWSER                                              │
│   [ ] Windows (86)    [ ] macOS (28)    [ ] Linux (14)          │
│   [ ] Chromium (110)                    [ ] Firefox (18)        │
├─────────────────────────────────────────────────────────────────┤
│ TAGS                                                            │
│   [x] Shopee (24)     [ ] Facebook (40)   [ ] Crypto (12)       │
├─────────────────────────────────────────────────────────────────┤
│ [ Reset ]                                          [ Apply (24) ]│
└─────────────────────────────────────────────────────────────────┘
```

---

### 4.3. Multi-Level Sort Popover (Khắc phục "Sort kém")
*Hỗ trợ cấu hình tối đa 4 tầng sort ưu tiên, cho phép hoán đổi vị trí (Move Up/Down), đảo chiều Asc/Desc, và xóa từng điều kiện.*

```text
┌─────────────────────────────────────────────────────────────┐
│ SORT CRITERIA                                       [Reset] │
├─────────────────────────────────────────────────────────────┤
│ 1. [ Status         [v] ]  [ [LuArrowDown] Desc [v] ]  [x]  │
│    (Đưa các profile đang hoạt động lên trên cùng)           │
│                                                             │
│ 2. [ Profile Name   [v] ]  [ [LuArrowUp]   Asc  [v] ]  [x]  │
│    (Sắp xếp theo thứ tự bảng chữ cái A-Z)                   │
│                                                             │
│ 3. [ Proxy Latency  [v] ]  [ [LuArrowUp]   Asc  [v] ]  [x]  │
│    (Ưu tiên proxy có độ trễ ping thấp nhất)                 │
├─────────────────────────────────────────────────────────────┤
│ [LuPlus] Add Sort Field                                     │
└─────────────────────────────────────────────────────────────┘
```

---

### 4.4. Cột Tiêu Đề Bảng (Table Header Columns)
*Thể hiện trực tiếp trạng thái sort đa tầng trên từng cột, có vạch ngăn co giãn kích thước cột.*

```text
┌───┬───────────────────┬──────────────┬───────────────────┬──────────────┐
│[ ]│ Profile Name   ↑2 │ Status    ↓1 │ Proxy / VPN     ↕ │ Tags         │
│   │                 ║ │            ║ │                 ║ │            ║ │
└───┴───────────────────┴──────────────┴───────────────────┴──────────────┘
```
- `↓1`: Cột ưu tiên sort cấp 1 (Giảm dần / Descending).
- `↑2`: Cột ưu tiên sort cấp 2 (Tăng dần / Ascending).
- `↕`: Cột có thể sort nhưng chưa được kích hoạt.
- `║`: Drag handle cho phép thay đổi độ rộng cột (`TableColumnResizer`).

---

### 4.5. Floating Bulk Action Bar (Khi tích chọn Profile)
*Khi người dùng tích chọn ít nhất 1 dòng, thanh `DataTableActionBar` sẽ xuất hiện ở giữa phía dưới màn hình:*

```text
┌───────────────────────────────────────────────────────────────────────────────────────┐
│ [LuCheckSquare] 5 selected │ [LuPlay] Run  [LuSquare] Stop  [LuGlobe] Proxy  [LuTag] Tag  [LuTrash2] Delete │ [LuX] │
└───────────────────────────────────────────────────────────────────────────────────────┘
```

---

## 5. Quy tắc Tương tác & Phím tắt (Keyboard & State Rules)

1. **Focus & Keyboard Navigation**:
   - `/` hoặc `Ctrl+F`: Focus trực tiếp vào ô `Search profiles...`.
   - `Ctrl+W`: Mở dropdown Workspace Switcher.
   - `Escape`: Đóng popover đang mở; nếu đang ở bảng thì bỏ chọn toàn bộ profile đã chọn.
   - `ArrowUp / ArrowDown`: Di chuyển giữa các dòng trong bảng hoặc menu popover.
2. **Persistence (Lưu trạng thái)**:
   - Các cột đã ẩn/hiện, độ rộng cột (`columnWidths`), và thứ tự `sorting` được lưu vào Local Table Preferences (`useTablePreferences`) theo từng workspace.
3. **Empty & Search Result States**:
   - Nếu không có kết quả lọc: Hiển thị Empty Filter State với nút `Clear filters` và số lượng `0 of N profiles`.
   - Không hiển thị các thông báo kỹ thuật hoặc dữ liệu giả lập.

---

## 6. Danh mục Kiểm tra Thực thi (Acceptance Checklist)

- [ ] Workspace Switcher nằm ở Titlebar bên trái, không đè lên draggable window handle.
- [ ] Search input và Filter button nằm liền kề nhau trên Table Toolbar.
- [ ] Active chips xuất hiện đầy đủ ngay dưới hoặc cạnh ô tìm kiếm khi có bộ lọc hoạt động.
- [ ] Menu Multi-level Sort cho phép thêm ít nhất 3 tầng sắp xếp và cập nhật chỉ báo `↑1`, `↓2` trên cột.
- [ ] Toàn bộ emoji được thay thế bằng Lucide icon primitives từ `react-icons/lu`.
- [ ] Thanh hành động nổi `DataTableActionBar` xuất hiện mượt mà khi chọn nhiều bản ghi.
- [ ] Không có chuỗi trạng thái kỹ thuật dư thừa (loại bỏ `(Local-first)`).
