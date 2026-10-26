# DES-04 Reference Package: Seamless App Update Lifecycle Experience

Status: approved design specification & interaction contract  
Screen: Titlebar Update Anchor, In-App Notification Banner, and Release Notes Dialog  
Authority: [`docs/design/ui-authority.md`](../ui-authority.md), [`DESIGN.md`](../../DESIGN.md), [`DES-02-unified-switcher-action-hub.md`](DES-02-unified-switcher-action-hub.md)  
Last updated: 2026-10-26  

---

## 1. Mục Tiêu và Triệt Tiêu Vấn Đề (Problem Statement & Goals)

### 1.1. Thực trạng trước khi tối ưu (Current Flaws)
1. **Kiểm tra thụ động & thủ công (Manual Checking Friction):** Người dùng không biết có phiên bản mới trừ khi tự mở hộp thoại "Giới thiệu" (About Dialog) rồi click nút "Kiểm tra cập nhật".
2. **Toast góc màn hình thô kệch & che khuất (Intrusive Corner Toast):** Khi có bản mới hoặc khi hoàn tất tải, một khối Toast vuông trôi nổi ở góc trên trái (`top-left`) hoặc dưới phải với z-index cực đại (`99999`), đè trực tiếp lên thanh điều hướng Breadcrumb, làm che khuất các nút bấm vận hành chính và tạo cảm giác chắp vá, gián đoạn công việc.
3. **Mù thông tin tiến trình tải ngầm (Silent Download Blindness):** Ứng dụng tải file cài đặt nền nhưng không cung cấp chỉ báo tiến trình trực quan (dung lượng, phần trăm, tốc độ tải). Người dùng hoàn toàn không biết phần mềm đang tải hay bị treo.
4. **Thiếu ngữ cảnh & Changelog (Zero Release Context):** Toast chỉ hiển thị `v3.0.13 → v3.0.14` khô khốc kèm nút "Restart now", không cho người dùng biết bản cập nhật có tính năng gì mới hay sửa lỗi quan trọng nào để họ quyết định khởi động lại.

### 1.2. Mục tiêu thiết kế mới (Target Goals)
1. **Tự động hóa kiểm tra ngầm (Silent Scheduled Check):** Ứng dụng tự động kiểm tra định kỳ (mỗi 4 giờ và 15 giây sau khi mở app) mà không gây bất kỳ gián đoạn nào.
2. **Neo chỉ báo không xâm lấn (Zero-Intrusive Header Anchor):** Tích hợp nút trạng thái cập nhật trực tiếp trên thanh Header (`HomeHeader`), đặt cạnh Avatar người dùng. Chỉ xuất hiện khi có sự kiện cập nhật; khi ở bản mới nhất, thanh header hoàn toàn sạch bóng.
3. **Dải Banner mỏng nội trang (Inline Update Banner `h-8`):** Khi bản cập nhật đã sẵn sàng, hiển thị dải thông báo phẳng mỏng cao `32px` nằm cố định ngay dưới Header, hài hòa với giao diện và cho phép người dùng chủ động chọn "Để sau" hoặc "Khởi động lại ngay".
4. **Hộp thoại Thông tin Bản cập nhật chuyên nghiệp (Release Notes Dialog):** Cho phép xem Changelog, dung lượng tải, tiến trình tải real-time và trạng thái xác thực chữ ký bảo mật Ed25519.

---

## 2. Chu Trình Vòng Đời Cập Nhật (The Update Lifecycle States)

```text
[1. IDLE (Bản mới nhất)]
       │
       ▼ (Tự động kiểm tra ngầm hoặc click menu)
[2. CHECKING...] ──(Không có bản mới)──► [Thông báo ngắn: "Bạn đang dùng bản mới nhất"]
       │
       ▼ (Phát hiện version mới)
[3. UPDATE AVAILABLE] 
       │ ──► Header hiển thị: [ ⬆️ Bản mới v3.0.14 ]
       │
       ▼ (Tải ngầm tự động theo cài đặt hoặc user bấm Tải)
[4. DOWNLOADING (Tải ngầm)]
       │ ──► Header hiển thị: [ ⬇️ Đang tải 68% • 2.4 MB/s ]
       │
       ▼ (Tải xong + Xác minh chữ ký Ed25519 thành công)
[5. UPDATE READY (Sẵn sàng)]
       │ ──► Header hiển thị: [ 🟢 Khởi động lại để cập nhật ]
       │ ──► Hiển thị dải Banner mỏng nội trang (h-8)
       │
       ▼ (User bấm "Khởi động lại ngay" hoặc tắt app)
[6. RESTART & APPLY] ──► Tự động chuyển giao phiên và khởi động lại với version mới
```

---

## 3. Sơ Đồ Bố Cục ASCII (Master Layouts)

### 3.1. Vị Trí Neo Chỉ Báo Trên Titlebar Header (Header Anchor)

```text
+-------------------------------------------------------------------------------------------------------------------------+
| [TITLEBAR / CONTEXT HEADER]                                                                                   [-][口][X]|
| [LuBug] BugLogin / Profiles │ [ 🏢 BugMedia / Dev WS │ [Admin] [v] ] │ [ [+ Mới] [v] ]                                          |
|                                                                                                                         |
| (Khu vực kéo cửa sổ drag region)                       [ (A) ⬇️ Đang tải 68% ]  [ (B) 🟢 Cập nhật v3.0.14 ]  [ 👤 Alex ] |
+-------------------------------------------------------------------------------------------------------------------------+
```

- **(A) Trạng thái đang tải (Downloading):**
  - Pill nhỏ gọn: `h-6.5 px-2.5 rounded-full bg-primary/10 text-primary border border-primary/20 text-[11px] font-mono flex items-center gap-1.5`.
  - Icon quay nhẹ hoặc mũi tên tải xuống, hiển thị phần trăm: `⬇️ 68%`.
  - Click vào mở Dialog chi tiết xem tiến trình tải đầy đủ.
- **(B) Trạng thái đã tải xong & Đã xác thực (Update Ready):**
  - Pill nổi bật màu ngọc lục bảo: `h-6.5 px-2.5 rounded-full bg-emerald-500/15 text-emerald-400 border border-emerald-500/30 text-[11px] font-medium flex items-center gap-1.5`.
  - Chấm tròn xanh lá animate pulse nhẹ: `🟢 Cập nhật v3.0.14`.
  - Click vào mở Popup xác nhận khởi động lại.

---

### 3.2. Dải Banner Mỏng Nội Trang (Inline Notification Banner `h-8: 32px`)

Xuất hiện mượt mà ngay bên dưới Header khi bản cập nhật đã sẵn sàng khởi động lại:

```text
+-------------------------------------------------------------------------------------------------------------------------+
| [INLINE UPDATE BANNER (h-8: 32px, bg-emerald-950/30 border-b border-emerald-500/20)]                                    |
|  ✨ BugLogin v3.0.14 đã sẵn sàng! Sửa 3 lỗi bảo mật & Tối ưu tốc độ sync profile.     [Xem điểm mới] [Để sau] [Khởi động lại] [X]|
+-------------------------------------------------------------------------------------------------------------------------+
| [SUB-HEADER TABS: ĐIỀU HƯỚNG TẬP TRUNG]                                                                                 |
|   [ 🌐 Profiles (128) ]   [ 📁 Groups (6) ]   [ 👥 Members (12) ]   [ 🛡️ Proxies (24) ]   [ 📜 Audit Log ]   [ ⚙️ Settings ] |
+-------------------------------------------------------------------------------------------------------------------------+
```

- **Cấu trúc Banner:**
  - Chiều cao cố định: `h-8 (32px)`, padding ngang `px-3`.
  - Icon: `LuSparkles` hoặc `LuCheckCircle2` màu `text-emerald-400`.
  - Nội dung text: Font-size `12px`, thông điệp súc tích, tóm tắt 1 dòng giá trị của bản cập nhật.
  - Nhóm nút thao tác:
    - `[Xem điểm mới]`: Nút ghost text-xs gạch chân nhẹ, mở dialog Changelog.
    - `[Để sau]`: Ẩn banner cho phiên làm việc hiện tại, chỉ giữ pill nhỏ trên Header.
    - `[Khởi động lại ngay]`: Nút primary xanh ngọc nhỏ (`h-6 px-2.5 text-xs`), click áp dụng cập nhật tức thì.
    - `[X]`: Đóng banner.

---

### 3.3. Hộp Thoại Chi Tiết Bản Cập Nhật (Release Notes & Progress Dialog)

Kích thước chuẩn: `max-w-lg (520px)`, hiển thị khi click vào Pill trên Header hoặc nút `[Xem điểm mới]`:

```text
┌─────────────────────────────────────────────────────────────────────────────────┐
│ [LuSparkles] Bản Cập Nhật Phần Mềm Mới                              [X Đóng]     │
├─────────────────────────────────────────────────────────────────────────────────┤
│                                                                                 │
│   ┌────────┐   BugLogin Desktop v3.0.14                                         │
│   │ [LOGO] │   Phiên bản hiện tại: v3.0.13 ➔ Mới nhất: v3.0.14 [Bản Chính Thức] │
│   └────────┘   Ngày phát hành: 26/10/2026 • Dung lượng gói: 42.8 MB             │
│                                                                                 │
│ ┌─────────────────────────────────────────────────────────────────────────────┐ │
│ │ ĐIỂM MỚI TRONG BẢN CẬP NHẬT NÀY (CHANGELOG)                                  │ │
│ │                                                                             │ │
│ │  ✨ TÍNH NĂNG MỚI:                                                          │ │
│ │    • Hợp nhất Switcher và Nút Thêm (+) phân quyền theo vai trò (DES-02).    │ │
│ │    • Dải điều hướng tập trung và Inline Metrics Ribbon siêu gọn (DES-03).   │ │
│ │    • Hỗ trợ quản lý phân quyền thành viên theo từng nhóm profile riêng biệt.│ │
│ │                                                                             │ │
│ │  🛡️ BẢO MẬT & HIỆU NĂNG:                                                   │ │
│ │    • Nâng cấp cơ chế xác thực chữ ký gói cài đặt Ed25519 chống giả mạo.     │ │
│ │    • Tăng 40% tốc độ khởi động trình duyệt Wayfern trên Windows 11.        │ │
│ │    • Khắc phục triệt để lỗi xung đột tiến trình nền khi chạy đồng thời.     │ │
│ └─────────────────────────────────────────────────────────────────────────────┘ │
│                                                                                 │
│   TIẾN TRÌNH TẢI XUỐNG:                                                         │
│   [===============================>          ] 74% (31.6 MB / 42.8 MB)          │
│   Tốc độ tải: 4.8 MB/s • Thời gian còn lại: ~2 giây                             │
│                                                                                 │
│   🔒 Xác minh an toàn: Chữ ký số Ed25519 hợp lệ (Release Feed: Verified)       │
│                                                                                 │
├─────────────────────────────────────────────────────────────────────────────────┤
│ [v] Tự động tải xuống các bản cập nhật trong tương lai                           │
│                                           [Để sau]  [🚀 Khởi động lại để cập nhật]│
└─────────────────────────────────────────────────────────────────────────────────┘
```

---

## 4. Đặc Tả Chi Tiết Các Khối Giao Diện

### 4.1. Khối Header Update Pill (Nằm trên `HomeHeader`)

| Trạng thái | Hiển thị Visual | Hành vi tương tác |
|---|---|---|
| **Đang kiểm tra** | Spinner nhỏ `size-3` xoay nhẹ | Tooltip: "Đang kiểm tra cập nhật..." |
| **Đang tải ngầm** | `[ ⬇️ 68% • 2.4 MB/s ]` (pill xanh dương nhạt) | Click: Mở hộp thoại xem chi tiết tiến trình tải |
| **Sẵn sàng cài** | `[ 🟢 Cập nhật v3.0.14 ]` (pill xanh ngọc emerald) | Click: Mở popup xác nhận khởi động lại ngay |
| **Lỗi tải/Chữ ký** | `[ ⚠️ Lỗi cập nhật ]` (pill hổ phách) | Click: Mở thông báo chi tiết mã lỗi & nút Thử lại |
| **Bản mới nhất** | Ẩn hoàn toàn khỏi Titlebar | Không chiếm diện tích |

### 4.2. Dải Banner Nội Trang (`InlineUpdateBanner`)
- **Tự động biến mất:** Khi bấm `[Để sau]` hoặc `[X]`, banner tự thu gọn với animation mượt mà `duration-200 ease-out`.
- **Không chặn thao tác:** Banner đẩy nhẹ nội dung bên dưới xuống `32px` một cách tự nhiên, không che bất kỳ nút bấm hay dòng dữ liệu nào (khác hoàn toàn với Toast dạng floating đè lên UI).
- **Phím tắt hỗ trợ:** Nhấn `Ctrl+Alt+U` để kích hoạt nhanh hành động Khởi động lại & Cập nhật.

### 4.3. Hộp thoại Changelog (`AppUpdateDialog`)
- **Chế độ khi Đang tải:** Nút chính hiển thị `[Đang tải...] (disabled)`, thanh Progress bar chuyển động mượt với tốc độ MB/s thực tế.
- **Chế độ khi Đã tải xong:** Nút chính chuyển thành `[🚀 Khởi động lại để cập nhật] (variant="default")`, tự động focus để người dùng có thể nhấn `Enter` kích hoạt ngay.
- **Xác thực chữ ký số:** Luôn hiển thị dòng xác nhận `Ed25519 signature verified` để tạo sự an tâm tuyệt đối cho người dùng B2B.

---

## 5. Ma Trận Xử Lý Lỗi & Ngoại Lệ (Resilience & Edge Cases)

```text
┌────────────────────────┬──────────────────────────────────────────┬────────────────────────────────────────────┐
│ Tình huống ngoại lệ    │ Cách hiển thị giao diện                  │ Cơ chế tự phục hồi                         │
├────────────────────────┼──────────────────────────────────────────┼────────────────────────────────────────────┤
│ Mất mạng khi đang tải  │ Pill chuyển `[ ⚠️ Mất kết nối ]`        │ Tự động thử lại (retry) sau 30s khi có mạng│
│ Sai chữ ký số Ed25519  │ Dialog cảnh báo đỏ: "Gói cài đặt không    │ Hủy file hỏng, kích hoạt tải lại từ đầu    │
│                        │ hợp lệ hoặc bị can thiệp"                │                                            │
│ Đang có profile chạy   │ Banner cảnh báo: "Đang có 2 profile đang │ Đề xuất: [Đóng profiles & Cập nhật] hoặc   │
│                        │ mở. Cần đóng trước khi cập nhật"         │ [Cập nhật khi thoát ứng dụng]              │
│ Cài qua Package Manager│ Nút đổi thành: `[Sao chép lệnh upgrade]` │ Hỗ trợ `winget upgrade` / `brew upgrade`   │
│ (winget / brew)        │                                          │                                            │
└────────────────────────┴──────────────────────────────────────────┴────────────────────────────────────────────┘
```

---

## 6. So Sánh Toàn Diện: Trải Nghiệm Cũ vs Thiết Kế Mới

| Tiêu chí | Cơ chế Cũ (Toast góc) | Thiết Kế Mới DES-04 (Header Pill + Banner + Dialog) |
|---|---|---|
| **Vị trí hiển thị** | Nổi góc trên trái `top-left`, đè lên breadcrumb | Tích hợp thẳng vào Header Titlebar & Banner mỏng `h-8` |
| **Độ xâm lấn** | Che khuất nội dung, gây khó chịu cho người dùng | Không xâm lấn, dạt dòng tự nhiên, có thể đóng/thu gọn |
| **Thông tin tiến trình** | Mù tiến trình (chỉ báo "Starting update") | Progress bar real-time: %, MB/MB, tốc độ tải MB/s |
| **Ngữ cảnh bản cập nhật**| Chỉ có 2 số version khô khốc `v1 -> v2` | Danh sách Changelog đầy đủ (Tính năng mới, Vá lỗi, Bảo mật)|
| **Độ tin cậy bảo mật** | Không hiển thị trạng thái chữ ký số | Xác thực Ed25519 rõ ràng, tạo sự tin cậy chuẩn doanh nghiệp |
| **Kiểm tra tự động** | Thụ động, chủ yếu phụ thuộc check thủ công | Tự động kiểm tra ngầm định kỳ, không làm phiền người dùng |
