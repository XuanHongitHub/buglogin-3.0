# BugLogin — Đặc tả chính UI/UX quản trị tổ chức, tài khoản và tài nguyên

Ngày: 2026-10-26
Trạng thái: đặc tả mục tiêu theo quyết định của người dùng; chưa phải bằng chứng triển khai hoặc release.
Phạm vi: Organization → Workspace → Members / Groups / Profiles; Platform Admin; Tài khoản của tôi.
Thẩm quyền: quyết định người dùng trong phiên hiện tại; [UI authority](../../design/ui-authority.md); Donut upstream; hợp đồng quyền backend.
Thay thế: bố cục/action quản trị trong [DES-03](../../design/references/DES-03-org-workspace-member-management.md) và các ASCII quản trị cũ mâu thuẫn với tài liệu này. DES-02 không được dùng để phục hồi switcher/action hub ở header mà người dùng đã yêu cầu bỏ.

## 1. Quyết định đã chốt

- Một source `F:\Herd\buglogin`, branch `main`. Không tạo worktree/nguồn UI thứ hai.
- Giữ shell Donut và switcher ở sidebar. Header chỉ có một breadcrumb; action chính ở toolbar nội dung bên cạnh nút làm mới. Không lặp switcher, tab, nút Mới, tiêu đề hoặc nút quay lại.
- Tables gọn, action từng hàng luôn một hàng. Không ép bảng cuộn ngang để chứa nút. Dùng một action trực tiếp thường dùng và menu dấu ba chấm có icon chuẩn, không emoji.
- Group/Profile trong quản trị chỉ phục vụ tổ chức tài nguyên, membership và quyền. Không có launch, stop, sync, browser download hoặc công cụ lifecycle chính.
- Bỏ action Lưu trữ/Archive ở mọi cấp và bulk. Không thêm trạng thái archived vào luồng này.
- Bỏ thiết lập sử dụng ngoại tuyến/Offline entitlement khỏi toàn bộ luồng này. Không đưa dung lượng lưu trữ/S3 quota vào bản thiết kế này; việc lưu dữ liệu profile lên S3 vẫn thuộc sync/backend.
- Single và bulk đều là luồng chính. Có sinh tài khoản tự động, dán Excel, CSV, sửa bảng trực tiếp và áp dụng theo cột; không buộc nhập từng tài khoản bằng tay.
- Tài khoản có tạo và sửa; membership có cấp/sửa/thu hồi riêng. Tài khoản cá nhân có sửa thông tin, đổi mật khẩu và quản lý phiên của chính mình.
- Form phù hợp nội dung: dialog gọn cho tạo/sửa đơn giản; panel rộng hai cột cho phân quyền; màn bulk rộng có bước nhập → kiểm tra → kết quả. Không dùng một drawer form chung cho mọi action.
- Chỉ triển khai control khi có vertical slice thật. Action thiếu API/persistence/quyền phải được hoàn thiện trong lớp sở hữu; không dùng toast giả, dead button hoặc dựng capability từ role ở frontend.

## 2. Điều hướng và ranh giới dữ liệu

```text
BugLogin — shell chung
├─ Profiles chính / tài nguyên vận hành / cài đặt app (ngoài phạm vi redesign này)
├─ Quản trị tổ chức
│  ├─ Danh sách tổ chức
│  └─ Tổ chức đã chọn
│     ├─ Workspaces
│     │  └─ Workspace đã chọn
│     │     ├─ Thành viên & quyền
│     │     ├─ Groups → thành viên có quyền / profiles thuộc nhóm
│     │     ├─ Profiles → nhóm / quyền hiệu lực / thông tin
│     │     ├─ Audit log
│     │     └─ Cài đặt
│     ├─ Danh bạ tài khoản (tổng hợp membership workspace)
│     ├─ Audit log
│     └─ Cài đặt tổ chức
├─ Platform Admin
│  ├─ Tổ chức
│  ├─ Workspaces
│  ├─ Tài khoản
│  ├─ Phân bổ hạn mức / quyền sử dụng
│  └─ Audit log
└─ Tài khoản của tôi
   ├─ Thông tin cá nhân
   ├─ Bảo mật
   └─ Phiên đăng nhập
```

Ranh giới dữ liệu: account identity dùng chung; organization membership là một record cha độc lập có status/revision; role và grants sử dụng tài nguyên nằm ở workspace. Đình chỉ thành viên Org chặn quyền truy cập xuống workspace nhưng giữ nguyên memberships/grants con để có thể khôi phục. Thao tác này không đình chỉ tài khoản toàn cục và không biến tài khoản hiện hữu thành tài khoản thuộc sở hữu độc quyền của một Org.

Những luồng mục tiêu chưa có contract đầy đủ (ví dụ username-only, sửa định danh, tự sinh bulk, chuyển Owner, batch quota hoặc xóa Org/WS) phải được kiểm tra rồi hoàn thiện contract/backend tương ứng trước khi chấp nhận UI. Tài liệu này không xác nhận chúng đã tồn tại.

## 3. Shell và danh sách tổ chức

```text
┌───────────────────┬─────────────────────────────────────────────────────────────────┐
│ BugLogin          │ Quản trị / Tổ chức                                              │
│                   ├─────────────────────────────────────────────────────────────────┤
│ [Org / Workspace] │ Tổ chức                                     [↻] [+ Tạo tổ chức] │
│                   │ [Tìm tên tổ chức, owner..............] [Vai trò ▾]               │
│ Profile           │                                                                 │
│ Mạng              │   TỔ CHỨC          CHỦ SỞ HỮU         VAI TRÒ         WS ACTIONS │
│ Tiện ích          │ ─────────────────────────────────────────────────────────────── │
│ ...               │   BugLogin Local   owner@company.com  Chủ sở hữu       3 [Vào][⋯]│
│                   │   BugMedia         admin@company.com  Theo quyền API  6 [Vào][⋯]│
│ Quản trị tổ chức  │                                                                 │
│ Platform Admin    │ Hiển thị 1–2 / 2                              [Trước] 1 [Sau]  │
│                   │                                                                 │
│ Tài khoản của tôi │                                                                 │
└───────────────────┴─────────────────────────────────────────────────────────────────┘
```

Tổ chức là đối tượng quản trị, không phải nơi phải có mọi bulk action. Không thêm checkbox nếu không có batch operation hợp lệ. Menu: vào workspaces, danh bạ tài khoản, sửa thông tin, chuyển Owner và xóa khi có policy/contract tương ứng. Các quyền tổ chức phải là quyền backend thực sự hỗ trợ; không tự tạo Org Admin membership.

```text
TẠO / SỬA TỔ CHỨC                       CHUYỂN CHỦ SỞ HỮU — action riêng
┌──────────────────────────────────┐    ┌────────────────────────────────────────┐
│ Chỉnh sửa tổ chức             × │    │ Chuyển chủ sở hữu                   × │
│                                  │    │ Hiện tại: Nguyễn An · an@company.com    │
│ Tên tổ chức *                    │    │ Chủ sở hữu mới *                       │
│ [BugLogin Local...............] │    │ [Tìm tài khoản đủ điều kiện.........▾] │
│                                  │    │ Quyền sẽ chuyển: [tóm tắt từ backend]  │
│ Chủ sở hữu: an@company.com       │    │ Quyền bạn giữ: [theo policy backend]    │
│           [Hủy] [Lưu thay đổi]   │    │       [Hủy] [Xem lại & xác nhận]       │
└──────────────────────────────────┘    └────────────────────────────────────────┘
```

Form nhỏ có ít trường là đúng khi model chỉ có ít trường. Không thêm slug, mô tả, toggle khóa hoặc quota tùy ý để làm form trông đầy. Trường bổ sung phải có mục đích và persistence thật.

## 4. Org → danh sách workspace

```text
┌ Tổ chức / BugLogin Local ───────────────────────────────────────────────────────────┐
│ [Workspaces] [Danh bạ tài khoản] [Audit log] [Cài đặt]                              │
│                                                                                    │
│ Workspaces                                         [↻] [+ Tạo workspace ▾]        │
│ [Tìm workspace...............................] [Sắp xếp ▾]                        │
│                                                                                    │
│ ☐ WORKSPACE          PROFILES / HẠN MỨC    MEMBERS / GHẾ             ACTIONS        │
│ ☐ Main Workspace          14 / 100              8 / 10           [Quản lý] [⋯]     │
│ ☐ Marketing                6 / 50               4 / 10           [Quản lý] [⋯]     │
│                                                                                    │
│ 2 đã chọn  [Điều chỉnh hạn mức] [Thêm thành viên] [Gỡ thành viên] [Bỏ chọn]        │
└────────────────────────────────────────────────────────────────────────────────────┘
```

Menu workspace: chỉnh sửa, thành viên & quyền, groups, profiles, cấp tài khoản, điều chỉnh hạn mức và xóa. Không có lưu trữ hoặc offline. Tạo nhiều workspace dùng bảng tên/hạn mức thành viên/hạn mức profile; có kiểm tra tổng phân bổ và quota còn lại, không tự tăng quota cha.

```text
TẠO / SỬA WORKSPACE
┌────────────────────────────────────────────────┐
│ Tạo workspace                               × │
│ Trong tổ chức BugLogin Local                   │
│                                                │
│ Tên workspace * [...........................] │
│ Profiles tối đa [100....]  Ghế thành viên [10] │
│ Còn có thể phân bổ: [dữ liệu thật từ backend]   │
│                                                │
│                      [Hủy] [Tạo workspace]     │
└────────────────────────────────────────────────┘
```

Sửa hạn mức phải cho thấy hiện dùng, cũ → mới, quota cha còn lại và các dòng không thể giảm. Hạn mức tổng Org chỉ xuất hiện khi backend có model và enforcement thật.

## 5. Danh bạ tài khoản và sửa tài khoản

```text
┌ BugLogin Local / Danh bạ tài khoản ────────────────────────────────────────────────┐
│ Tài khoản trong các workspace của tổ chức         [↻] [+ Tạo tài khoản ▾]          │
│ [Tìm tên, username, email................] [Workspace ▾] [Trạng thái membership ▾]│
│                                                                                    │
│ ☐ TÀI KHOẢN / ĐỊNH DANH     WORKSPACES ĐƯỢC CẤP         QUYỀN CỦA BẠN    ACTIONS │
│ ☐ Nguyễn An                Marketing, Sales            Quản lý quyền     [Sửa][⋯]│
│   nguyen.an · an@...                                                               │
│ ☐ Trần Linh                Marketing                   Quản lý quyền     [Sửa][⋯]│
│   tran.linh · chưa thêm email                                                      │
│                                                                                    │
│ 2 đã chọn [Sửa hàng loạt] [Cấp workspace] [Sửa quyền] [Gỡ membership] [Bỏ chọn]  │
└────────────────────────────────────────────────────────────────────────────────────┘
```

Menu Tạo: một tài khoản / nhiều tài khoản. Action Sửa mở chi tiết với tab Thông tin và Workspace & quyền. Sửa định danh chỉ khả dụng khi actor được phép account-level mutation. Quyền quản trị membership không tự cho phép sửa global identity, mật khẩu hoặc khóa global account. Admin cần xem rõ “quản lý quyền truy cập” thay cho “sửa tài khoản” khi chỉ được sửa membership.

```text
┌ Nguyễn An · nguyen.an ────────────────────────────────────────────────────────────┐
│ [Thông tin tài khoản] [Workspace & quyền] [Phiên*] [Audit]                         │
│                                                                                    │
│ Tên hiển thị  [Nguyễn An........................................]                 │
│ Username      [nguyen.an........................................]                 │
│ Email         [an@company.com...................................]                 │
│ Trường được sửa/khóa và lý do lấy theo quyền + policy.                             │
│                                                               [Lưu thay đổi]     │
├────────────────────────────────────────────────────────────────────────────────────┤
│ WORKSPACE & QUYỀN                                             [+ Cấp workspace]  │
│ Marketing    Trưởng nhóm   Marketing US, EU                [Sửa quyền] [Gỡ]       │
│ Sales        Thành viên    Sales Inbox                    [Sửa quyền] [Gỡ]       │
└────────────────────────────────────────────────────────────────────────────────────┘
* Phiên người khác chỉ hiển thị với quyền quản trị phiên tương ứng.
```

Sửa bulk có apply-to-column, dán Excel và CSV. Dùng account reference ổn định bên trong dữ liệu nhập để không gán nhầm người; UI hiển thị định danh rõ ràng. Ô trống mặc định giữ nguyên, xóa giá trị cần lệnh rõ. Email/username thay đổi cần quy trình xác minh, chống trùng và xử lý ảnh hưởng đăng nhập; không ghi đè ngay bằng bulk metadata update.

## 6. Tạo tài khoản đơn

```text
┌ Tạo tài khoản ──────────────────────────────────────────────────────── × ┐
│ THÔNG TIN ĐĂNG NHẬP                                                    │
│ Tên hiển thị *       [..............................................] │
│ Username *           [.................................] [Tự sinh]    │
│ Email                [.................................] (tùy chọn*) │
│ Mật khẩu ban đầu *   [••••••••••••••••••••••••] [Hiện] [Sinh riêng]  │
│                                                                        │
│ CẤP QUYỀN                                               [+ Workspace] │
│ Workspace       Vai trò                Phạm vi nhóm                    │
│ Marketing       [Thành viên ▾]         [Chọn groups ▾]                 │
│                                                                        │
│                                  [Hủy] [Kiểm tra & tạo tài khoản]     │
└────────────────────────────────────────────────────────────────────────┘
```

* Mục tiêu: username-only cho tài khoản nội bộ, email tùy chọn. Nếu contract hiện tại bắt email, triển khai tương thích username-only trước khi cho UI tạo; tuyệt đối không sinh email giả. Tên/username có gợi ý tự sinh, mật khẩu sinh riêng bằng CSPRNG trong lớp Rust có thẩm quyền; không dùng Math.random hoặc một mật khẩu chung mặc định. Ngưỡng chiều dài theo auth policy hiện hành.

Tạo scoped từ WS điền sẵn WS đó. Từ Org/Platform được chọn nhiều workspace trong scope cho phép. Identity và membership có kết quả riêng; nếu account đã được tạo nhưng membership lỗi, retry chỉ phần lỗi, không tạo identity trùng.

## 7. Tạo tài khoản hàng loạt — tự sinh / Excel / CSV

```text
┌ Tạo tài khoản hàng loạt ──────────────────────────────────────────────────────── × ┐
│ 1. Chuẩn bị tài khoản → 2. Cấp quyền → 3. Kiểm tra → 4. Kết quả                    │
│                                                                                    │
│ [Sinh tự động] [Dán từ Excel] [Nhập CSV]                                           │
│                                                                                    │
│ Số lượng             [20....]                                                     │
│ Username             [marketing-.......] Bắt đầu [001]                             │
│ Tên hiển thị         [Nhân viên Marketing {n}...............................]      │
│ Mật khẩu             [Sinh riêng cho từng tài khoản ▾]                            │
│                                                                                    │
│                                                       [Tạo bảng xem trước →]     │
└────────────────────────────────────────────────────────────────────────────────────┘

┌ Bảng tài khoản — có thể sửa trực tiếp ────────────────────────────────────────────┐
│ [Dán Excel] [Nhập CSV] [Tải mẫu] [Áp dụng cả cột ▾]                 [+ Thêm dòng] │
│                                                                                    │
│ ☐ USERNAME        TÊN HIỂN THỊ           EMAIL (TÙY CHỌN)     MẬT KHẨU       ×    │
│ ☐ marketing-001   Nhân viên Marketing 1  [................]  Đã sinh riêng  [×]   │
│ ☐ marketing-002   Nhân viên Marketing 2  [................]  Đã sinh riêng  [×]   │
│ ☐ marketing-003   Nhân viên Marketing 3  [................]  Đã sinh riêng  [×]   │
│                                                                                    │
│ [Sinh lại mật khẩu dòng chọn] [Bỏ dòng chọn]          [Hủy] [Tiếp tục: cấp quyền] │
└────────────────────────────────────────────────────────────────────────────────────┘

┌ Cấp quyền ────────────────────────────────────────────────────────────────────────┐
│ Áp dụng chung: Workspace [Marketing ▾] Vai trò [Thành viên ▾] Nhóm [US ▾] [Áp dụng]│
│                                                                                    │
│ TÀI KHOẢN          WORKSPACE         VAI TRÒ               PHẠM VI NHÓM            │
│ marketing-001      Marketing         [Trưởng nhóm ▾]       [US, EU ▾]              │
│ marketing-002      Marketing         [Thành viên ▾]        [US ▾]                  │
│ marketing-003      Marketing         [Thành viên ▾]        [US ▾]                  │
│                                                   [← Quay lại] [Kiểm tra →]      │
└────────────────────────────────────────────────────────────────────────────────────┘

┌ Kiểm tra trước khi tạo ───────────────────────────────────────────────────────────┐
│ TÀI KHOẢN          KIỂM TRA                 QUYỀN SẼ ĐƯỢC CẤP                    │
│ marketing-001      Hợp lệ                   Marketing · Trưởng nhóm · US, EU      │
│ marketing-002      Username đã tồn tại      [Đổi tên] [Chọn tài khoản hiện có]     │
│ marketing-003      Không được cấp role      [Sửa vai trò]                         │
│                                                                                    │
│ 1 hợp lệ · 2 cần xử lý                          [Quay lại] [Tạo 1 dòng hợp lệ]   │
└────────────────────────────────────────────────────────────────────────────────────┘

┌ Kết quả ──────────────────────────────────────────────────────────────────────────┐
│ TÀI KHOẢN          IDENTITY              MEMBERSHIP                 KẾT QUẢ         │
│ marketing-001      Đã tạo                Đã cấp Marketing           Thành công      │
│ marketing-004      Đã tạo                Cấp Marketing thất bại     [Thử cấp lại]   │
│                                                                                    │
│ [Tải thông tin bàn giao] [Tải báo cáo lỗi] [Thử lại mục lỗi]              [Đóng]   │
└────────────────────────────────────────────────────────────────────────────────────┘
```

Không tự chọn tài khoản hiện hữu khi trùng username/email; cần người dùng xem và chọn gán membership. Không reset mật khẩu tài khoản cũ trong luồng tạo. Secrets bàn giao chỉ có trong phiên kết quả có quyền, bị che mặc định, có hành động xuất/sao chép rõ; không đưa vào URL, localStorage, audit, telemetry hoặc log. Đóng/tải lại trang không được hứa lấy lại plaintext. Xuất credentials khác với xuất báo cáo thông thường; phải có thông báo dữ liệu nhạy cảm và kiểm soát quyền tương ứng. Batch có giới hạn từ backend, idempotency và kết quả bền vững đã redaction.

## 8. Thành viên workspace: thêm / mời / sửa / gỡ

```text
┌ BugLogin Local / Marketing ───────────────────────────────────────────────────────┐
│ [Thành viên & quyền] [Groups] [Profiles] [Audit log] [Cài đặt]                      │
│                                                                                    │
│ Thành viên & quyền                                    [↻] [+ Thêm thành viên ▾]  │
│ [Tìm tên, username, email................] [Vai trò ▾] [Trạng thái ▾]              │
│                                                                                    │
│ ☐ THÀNH VIÊN          VAI TRÒ         PHẠM VI NHÓM       MEMBERSHIP        ACTIONS │
│ ☐ Nguyễn An           Trưởng nhóm    Marketing US, EU    Hoạt động       [Sửa][⋯] │
│ ☐ Trần Linh           Thành viên     Marketing US        Hoạt động       [Sửa][⋯] │
│ ☐ Linh@example.com    Thành viên     Marketing EU        Đang mời         [⋯]     │
│                                                                                    │
│ 2 đã chọn [Đổi vai trò] [Gán/bỏ nhóm] [Đình chỉ] [Kích hoạt] [Gỡ] [Bỏ chọn]       │
└────────────────────────────────────────────────────────────────────────────────────┘
```

Menu Thêm: chọn từ danh bạ / mời bằng email / tạo tài khoản mới. Mỗi entry hỗ trợ một hoặc nhiều người trong chính luồng đó. Đình chỉ membership, gỡ membership và khóa global identity là ba hành động khác nhau. Pending invitations có gửi lại/hủy; không hiển thị các action active member lên invitation.

```text
┌ Thêm từ danh bạ ─────────────────────────────────────────────────────────────── × ┐
│ [Tìm tài khoản có thể cấp quyền...............................................] │
│ ☐ TÀI KHOẢN                        VAI TRÒ                 GROUPS                │
│ ☐ Nguyễn An · nguyen.an             [Trưởng nhóm ▾]         [US, EU ▾]           │
│ ☐ Trần Linh · tran.linh             [Thành viên ▾]          [US ▾]               │
│                                                     [Hủy] [Thêm 2 thành viên]   │
└────────────────────────────────────────────────────────────────────────────────────┘

┌ Mời bằng email ──────────────────────────────────────────────────────────────── × ┐
│ [Một người] [Nhiều người / Dán Excel / CSV]                                       │
│ EMAIL                       VAI TRÒ                   GROUPS                    │
│ [linh@example.com.......]   [Thành viên ▾]            [US ▾]                    │
│ [.......................]   [Thành viên ▾]            [EU ▾]                    │
│ [+ Thêm dòng]                                      [Hủy] [Kiểm tra & gửi lời mời]│
└────────────────────────────────────────────────────────────────────────────────────┘
```

Lời mời có trạng thái gửi/đã chấp nhận/hết hạn/đã hủy. Phải có kênh giao lời mời thật theo deployment; không báo đã gửi email khi chỉ tạo invitation record.

## 9. Phân quyền — rõ role, scope và hiệu lực

```text
┌ Quyền truy cập — Nguyễn An · nguyen.an ───────────────────────────────────────── × ┐
│ Workspace Marketing                                                               │
├──────────────────────────────────┬─────────────────────────────────────────────────┤
│ VAI TRÒ TRONG WORKSPACE           │ PHẠM VI NHÓM                                    │
│                                  │ [Tìm nhóm...................................]  │
│ ○ Quản trị viên                  │ ☑ Marketing US                    8 profiles   │
│   Quyền workspace theo policy    │ ☑ Marketing EU                    5 profiles   │
│ ● Trưởng nhóm                    │ ☐ Sales                           6 profiles   │
│   Quản lý phạm vi được giao      │                                                 │
│ ○ Vận hành viên                  │ QUYỀN HIỆU LỰC                                 │
│   [mô tả đúng capability thật]   │ [Danh sách quyền backend trả về]               │
│ ○ Thành viên                     │ Nguồn: role / nhóm / grant trực tiếp / deny     │
│   [mô tả đúng capability thật]   │ Hạn chế: [scope, expiry hoặc policy]             │
├──────────────────────────────────┴─────────────────────────────────────────────────┤
│ [Gỡ khỏi workspace]                                  [Hủy] [Kiểm tra & lưu quyền]│
└────────────────────────────────────────────────────────────────────────────────────┘
```

Role hiển thị theo thứ bậc được server định nghĩa, kèm mô tả, quyền và phạm vi. Owner hiển thị ở nơi thích hợp nhưng chuyển Owner có luồng riêng. Không mặc định Operator cao hơn Member hoặc hai role có quyền khác nhau nếu contract chưa chứng minh; phải rà soát và giải thích được khác biệt trước triển khai. Không cho grant một role/capability vượt grant ceiling. Lead chỉ quản lý nhóm và cấp dưới khi backend cho phép; chức danh không tự cấp quyền.

Bulk role/scope hỗ trợ apply chung hoặc sửa riêng từng dòng; preview hiệu lực cũ → mới, quyền kế thừa/deny không thể gỡ, danh sách dòng được phép và bị từ chối. Tự sửa quyền của mình, thao tác vào cấp ngang/cao hơn và chủ sở hữu cuối cùng phải theo server policy.

## 10. Groups và quản lý phạm vi

```text
┌ Groups — Marketing ───────────────────────────────────────────────────────────────┐
│ [Tìm nhóm...........................................] [↻] [+ Tạo nhóm ▾]        │
│                                                                                    │
│ ☐ NHÓM                 PROFILES       THÀNH VIÊN CÓ QUYỀN                 ACTIONS │
│ ☐ Marketing US              8         Nguyễn An, Trần Linh +2         [Quản lý][⋯]│
│ ☐ Marketing EU              5         Sarah +1                        [Quản lý][⋯]│
│                                                                                    │
│ 2 đã chọn [Sửa tên] [Cấp/gỡ thành viên] [Gán/bỏ profiles] [Xóa nhóm] [Bỏ chọn]    │
└────────────────────────────────────────────────────────────────────────────────────┘

┌ Marketing US ─────────────────────────────────────────────────────────────────────┐
│ [Thành viên có quyền] [Profiles thuộc nhóm]                           [Sửa tên]   │
│ [Tìm thành viên....................................]          [+ Cấp thành viên]│
│ THÀNH VIÊN                VAI TRÒ             NGUỒN QUYỀN                ACTIONS │
│ Nguyễn An                 Trưởng nhóm         Grant trực tiếp          [Thu hồi]│
│ Sarah                     Quản trị viên       Kế thừa workspace       [Xem quyền]│
└────────────────────────────────────────────────────────────────────────────────────┘
```

Tạo một nhóm dùng dialog tên; tạo nhiều nhóm dùng bảng tên có thể dán. Sửa bulk tên hỗ trợ prefix/suffix/find-replace hoặc bảng cũ → mới. Gán/bỏ profiles là bảng chọn, chỉ hiển thị profile trong workspace hợp lệ. Xóa nhóm phải nêu số profiles còn giữ, sẽ chuyển về chưa phân nhóm theo contract; không mặc nhiên xóa profile. Nếu contract khác phải sửa rõ trước triển khai.

## 11. Profiles trong quản trị — không lifecycle

```text
┌ Profiles — Marketing ─────────────────────────────────────────────────────────────┐
│ [Tìm profile........................] [Nhóm ▾] [Quyền truy cập ▾] [↻]             │
│                                                                                    │
│ ☐ PROFILE              NHÓM             QUYỀN TRUY CẬP                   ACTIONS │
│ ☐ Shopify US 01        Marketing US     4 thành viên                 [Quyền][⋯] │
│ ☐ Sales Inbox 01       Sales            2 thành viên                 [Quyền][⋯] │
│ ☐ Profile 03           Chưa phân nhóm   Quyền kế thừa workspace      [Quyền][⋯] │
│                                                                                    │
│ 2 đã chọn [Sửa thông tin] [Gán/bỏ nhóm] [Cấp/thu hồi quyền] [Xóa] [Bỏ chọn]       │
└────────────────────────────────────────────────────────────────────────────────────┘
```

Menu: xem thông tin, sửa metadata, gán/bỏ group, xem/cấp/thu hồi grants, xóa. Không thêm create/browser/fingerprint/launch/stop/sync ở đây; tạo/cấu hình runtime ở Profiles chính. Sửa metadata đơn gồm những trường thật đã có (ví dụ tên, ghi chú, tags); bulk hỗ trợ apply-to-column và preview. Grant dialog cho chọn người/nhóm, capability được phép, scope, expiry nếu contract có; phân biệt direct và inherited. Không hiển thị số thành viên hiệu lực suy đoán từ số grants.

## 12. Platform Admin

```text
┌───────────────────┬─────────────────────────────────────────────────────────────────┐
│ Platform Admin    │ Platform Admin / Tài khoản                                     │
│                   ├─────────────────────────────────────────────────────────────────┤
│ Tổ chức           │ Tài khoản                                 [↻] [+ Tạo tài khoản ▾]│
│ Workspaces        │ [Tìm tên/username/email.........] [Org ▾] [Trạng thái ▾]       │
│ Tài khoản         │                                                                 │
│ Phân bổ hạn mức   │ ☐ TÀI KHOẢN       TỔ CHỨC/WORKSPACE        IDENTITY     ACTIONS │
│ Audit log         │ ☐ Nguyễn An       BugMedia / Marketing    Hoạt động   [Xem][⋯] │
│                   │   nguyen.an                                                     │
│                   │ ☐ Trần Linh       BugMedia / Sales        Bị khóa     [Xem][⋯] │
│                   │   tran.linh                                                     │
│                   │                                                                 │
│                   │ 2 chọn [Sửa bulk] [Cấp quyền] [Khóa/mở] [Thu hồi phiên] [Xuất]│
└───────────────────┴─────────────────────────────────────────────────────────────────┘
```

Admin dùng cùng components bảng/form/domain, scope toàn nền tảng theo quyền thật. Tạo/sửa tài khoản đơn và bulk dùng luồng ở §5–7; chọn Org/WS rõ ràng. Admin không được đọc mật khẩu hiện tại. Administrative password reset chỉ là action riêng nếu contract hỗ trợ, có audit và policy; không đưa plaintext vào bảng sửa tài khoản.

```text
┌ Platform Admin / Phân bổ hạn mức ─────────────────────────────────────────────────┐
│ [Tìm tổ chức hoặc workspace...................................] [↻]             │
│                                                                                    │
│ ☐ TỔ CHỨC / WORKSPACE          PROFILES DÙNG / GIỚI HẠN    MEMBERS DÙNG / GHẾ       │
│   BugMedia                    [tổng nếu contract có]      [tổng nếu contract có]    │
│ ☐   Marketing                          120 / 200                 12 / 20          │
│ ☐   Sales                               80 / 100                  8 / 10          │
│                                                                                    │
│ 2 đã chọn                                       [Điều chỉnh phân bổ hàng loạt]   │
└────────────────────────────────────────────────────────────────────────────────────┘

┌ Điều chỉnh hạn mức ──────────────────────────────────────────────────────────── × ┐
│ Áp dụng chung: Profiles [......] Ghế [......]                    [Áp dụng cột]   │
│ WORKSPACE       ĐANG DÙNG        GIỚI HẠN CŨ → MỚI           KIỂM TRA             │
│ Marketing       120 profiles    200 → [300]                  Hợp lệ             │
│ Sales            80 profiles    100 → [50.]                  Thấp hơn mức dùng  │
│                                                     [Hủy] [Kiểm tra & áp dụng] │
└────────────────────────────────────────────────────────────────────────────────────┘
```

Các danh sách Org/WS toàn nền tảng dùng mẫu §3–4, có thêm bộ lọc owner/Org và action phù hợp global scope. Không tạo dashboard KPI lớn, health hư cấu, storage quota, offline toggle hoặc control chưa có backend.

## 13. Tài khoản của tôi

```text
┌ Tài khoản của tôi ────────────────────────────────────────────────────────────────┐
│ [Thông tin cá nhân] [Bảo mật] [Phiên đăng nhập]                                    │
│                                                                                    │
│ THÔNG TIN CÁ NHÂN                                                                  │
│ Tên hiển thị  [Nguyễn An........................................]                 │
│ Username      [nguyen.an........................................]                 │
│ Email         [an@company.com...........................] [Đã xác minh]          │
│ Cho sửa/khóa từng trường theo policy; thay email có xác minh khi cần.               │
│                                                               [Lưu thay đổi]     │
│                                                                                    │
│ BẢO MẬT                                                                            │
│ Mật khẩu hiện tại [.............................]                                │
│ Mật khẩu mới      [.............................]                                │
│ Xác nhận          [.............................]                  [Đổi mật khẩu]│
│                                                                                    │
│ PHIÊN ĐĂNG NHẬP                                                                    │
│ Thiết bị này      [tên đã xác minh]       Lần dùng gần nhất      Phiên hiện tại     │
│ Thiết bị khác     [tên đã xác minh]       Lần dùng gần nhất      [Đăng xuất]       │
│                                             [Đăng xuất các thiết bị khác]        │
└────────────────────────────────────────────────────────────────────────────────────┘
```

Tabs là các view riêng, không xếp cả ba section thành một trang dài như sơ đồ trình bày. Profile self-service không cho tự sửa role, scope, trạng thái hoặc hạn mức. Không hiển thị thiết bị/IP suy đoán hay fallback ID thành tên người.

## 14. Ma trận action đơn / bulk và quyền

| Đối tượng | Single | Bulk | Quyền và giới hạn |
|---|---|---|---|
| Org | Tạo, sửa tên, vào WS/danh bạ, chuyển Owner, xóa | Không ép bulk Org; chỉ thêm batch thật được duyệt | Org owner/platform policy; chuyển Owner riêng; xóa cần kiểm tra phụ thuộc |
| Workspace | Tạo, sửa tên/hạn mức, xóa | Tạo nhiều, sửa hạn mức, thêm/gỡ members | Scope Org/WS và quota cha; không nâng quota bằng UI |
| Account identity | Tạo, sửa thông tin, khóa/mở, thu hồi phiên khi được phép | Tạo tự sinh/Excel/CSV; sửa metadata; khóa/mở; thu hồi phiên | Account-level authority khác membership authority; email/username/password theo luồng riêng |
| Membership | Thêm từ danh bạ, sửa role/scope, đình chỉ/kích hoạt, gỡ | Tất cả cùng preview từng người/workspace | Role ceiling + effective grants + scope; deny-by-default |
| Invitation | Mời, gửi lại, hủy | Mời nhiều, gửi lại/hủy các invitation hợp lệ | Trạng thái invitation và giao lời mời thật |
| Group | Tạo, sửa tên, cấp/gỡ member, gán/bỏ profiles, xóa | Tạo nhiều, sửa tên, cấp/gỡ member, gán/bỏ profiles, xóa | Trong workspace và scope nhóm được quản lý; kế thừa hiển thị read-only |
| Profile quản trị | Sửa metadata, gán/bỏ group, cấp/thu hồi grant, xóa | Tương tự cho nhiều profile | Không lifecycle; kiểm tra authority và hậu quả xóa |
| Tài khoản của tôi | Sửa thông tin, đổi password, revoke session | Thu hồi các phiên khác | Chỉ bản thân; không self-escalation |

Tên vai trò không đủ để kết luận quyền. Backend xác minh actor/action/target/scope/grant ceiling; Org owner quản lý các WS trong Org theo contract; WS Admin trong WS; Lead trong scopes được grant; Member/Operator theo capability thực tế. Frontend trình bày permitted actions và lý do bị hạn chế, không là authority.

## 15. Hợp đồng tương tác bulk dùng chung

```text
Chọn dòng / nhập bảng
      ↓
Chọn thay đổi: áp dụng chung / từng dòng / dán Excel / CSV
      ↓
Kiểm tra ở backend: quyền + scope + trùng + quota + revision
      ↓
Preview: cũ → mới · hợp lệ / cần sửa / bị từ chối / không thay đổi
      ↓
Xác nhận số mục và hậu quả thực tế
      ↓
Kết quả: thành công / thất bại / bỏ qua / không đổi
      ↓
Thử lại đúng mục lỗi; tải báo cáo đã loại secrets
```

- Bulk mutation bounded, idempotent, revalidate quyền/revision tại commit. Không xem validation preview là quyền vĩnh viễn.
- Không auto-check tất cả kết quả ở trang khác. Chọn tất cả trang phải hiển thị rõ phạm vi, số lượng và filter snapshot; thay filter không giữ selection mơ hồ.
- Bảng nhập giữ bản nháp khi validation lỗi. Dán hỗ trợ ô/tab/newline, quoted CSV và báo lỗi đúng cột; không split comma đơn giản rồi coi là parser hoàn chỉnh.
- Các thay đổi phá hủy/quyền có preview riêng. Không tự xóa, thu hồi hoặc khóa ngay khi chọn menu.
- API batch chưa có phải hoàn thiện semantics hoặc dùng batch orchestration đúng kiến trúc, có kết quả từng mục. Không vòng lặp frontend bắn request rồi chỉ toast “xong”.
- Account và membership partial success phải phân biệt rõ; retry không tạo credential/account trùng.

## 16. Trạng thái, ergonomics và điều kiện nghiệm thu

| Trạng thái | Cách hiển thị |
|---|---|
| Loading | Giữ shell/context và tiêu đề; skeleton bảng đúng kích thước |
| Empty | CTA phù hợp quyền; giải thích tài nguyên đang trống |
| Filter empty | Giữ bộ lọc và nút xóa lọc |
| Error | Giữ dữ liệu nhập; lỗi theo trường/dòng; retry đúng thao tác |
| Forbidden | Nêu quyền thiếu trong scope; không lộ dữ liệu vượt quyền |
| Network unavailable | Giữ dữ liệu biết trước và freshness; mutation remote giải thích không khả dụng; không có toggle offline |
| In progress | Trạng thái thật, chống submit lặp, theo dõi kết quả khi rời view |
| Conflict | Cũ/mới và refresh/resolve; không ghi đè silent |
| Success / partial | Bảng cập nhật từ kết quả thật; kết quả bulk giữ từng dòng |

- Dùng shared primitives/token/icon Donut, i18n VI/EN; icon-only controls có accessible name.
- Header/breadcrumb một lần; một primary action đúng ngữ cảnh; không KPI cards lớn hoặc text nhắc prototype trong production.
- Action column một hàng, menu popover ổn định; không dùng native select giả menu action.
- Desktop root không cuộn toàn cửa sổ. Nội dung/table có vùng cuộn; hạn chế cột phụ thay vì ép horizontal scroll. Bulk grid có thể cuộn vùng dữ liệu nếu cần, pin identity và lỗi/action để luôn thao tác được.
- Dialog vừa nội dung, quyền hai cột khi đủ rộng, bulk rộng; co xuống một cột khi cửa sổ nhỏ. Không khoảng trống lớn do ép mọi form full-height.
- Keyboard: tab/focus-visible, Escape, focus trap/return focus, điều hướng bảng; đóng form có thay đổi cần xử lý bản nháp hợp lý; secrets không lưu bền trong draft.
- Kiểm tra các role và scope bằng UI lẫn API; self-service và cross-tenant denial; trùng account; quota; partial success/idempotency; không secrets trong logs.
- Screenshot và interaction QA ở desktop rộng/hẹp, long labels, empty/error/forbidden/bulk partial; không coi typecheck/build là bằng chứng hoàn tất UI.
- Bản HTML/prototype mới chỉ được dùng review thiết kế, không là production evidence. Prototype cũ `management-ui-test` đã bị loại bỏ.

## 17. Thứ tự triển khai theo đặc tả này

1. Đối chiếu model/API và lập capability/action matrix thực tế cho mỗi scope; chốt role semantics và account self-service policy.
2. Tạo bản review đầy đủ các màn chính và action ở tài liệu này, dùng shared design system và đủ trạng thái tương tác; không tiếp tục prototype generic drawer đã bỏ.
3. Nối từng vertical slice: Org/WS → identity provisioning đơn/bulk → membership/grants → groups/profile governance → Platform Admin/self-service.
4. Kiểm tra backend permissions và interaction/screenshot; cập nhật evidence thực tế trước release. Không tự đổi main source hoặc nhập WIP của session khác.
