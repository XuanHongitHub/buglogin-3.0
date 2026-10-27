# Codex & Agent Collaboration Log

> **Protocol Rule**: File này duy trì nhật ký trao đổi và đồng bộ công việc liên tục giữa các agent (Antigravity & Codex). Giữ tối đa **10 phiên gần nhất** theo cơ chế sliding window (khi đạt phiên thứ 11 thì phiên cũ nhất sẽ được lược bỏ hoặc tổng hợp vào phần tóm tắt).

## Quy tắc thực thi của Codex & Antigravity

- Khi người dùng đã giao việc rõ ràng, thực hiện việc đó; chỉ đưa plan khi người dùng yêu cầu plan.
- Trước thao tác dọn dữ liệu, xác minh đường dẫn tuyệt đối và tiến trình giữ file; chỉ đụng đúng build output BugLogin đã xác minh.
- Nếu execution policy từ chối thao tác, dừng thử lại cùng hành động, nói rõ lệnh nào bị chặn và lý do được trả về; không vòng qua policy bằng shell hay công cụ khác. Tiếp tục phần việc độc lập có thể làm an toàn.
- Giữ goal đang hoạt động qua các lượt; không báo hoàn thành khi chưa có bằng chứng chạy thực tế và không lặp lại status thay cho tiến độ.

---

## Tóm tắt Lịch sử & Context Chung
- **Dự án**: BugLogin 2.0 (Kiến trúc Donut Upstream + Rust Backend).
- **Thanh lọc toàn diện UI/UX**:
  - Đã xóa sạch các skills `.md` cũ cồng kềnh/lỗi thời (`ui-ux-pro-max`), thay mới bằng 2 bộ skills chuẩn hóa cao cấp từ GitHub:
    + [`ui-ux-designer`](.agents/skills/ui-ux-designer/SKILL.md): IA, WCAG 2.1 AA, OKLCH Tokens, Ma trận 8 trạng thái UI.
    + [`desktop-operations-ui`](.agents/skills/desktop-operations-ui/SKILL.md): Chuẩn Desktop Donut Upstream, Tauri Local-First, Compact B2B density.
  - Đã khóa hai gói đặc tả kiến trúc mới, xóa bỏ hoàn toàn bố cục phân tán và thanh KPI thô kệch:
    + [`DES-02`](docs/design/references/DES-02-unified-switcher-action-hub.md): Unified Context Switcher & Action Hub (+).
    + [`DES-03`](docs/design/references/DES-03-org-workspace-member-management.md): Workspace Resource & Member Management (Dải trạng thái siêu mỏng inline `h-7`, Mô hình nhân sự 3 tầng, Quản lý Nhóm/Proxy/Audit).

---

## Phiên 16: Tinh Lọc Toàn Diện UI/UX Rules, Đồng Bộ Cây Quản Trị & Khóa Chuẩn Kiến Trúc Mới
- **Thời gian**: 2026-10-26
- **Người thực hiện**: Antigravity (AGY) & User
- **Phạm vi hoàn tất**:
  1. **Nâng cấp Hệ thống Agent Skills**:
     - Thiết lập `.agents/skills/ui-ux-designer/SKILL.md` và `.agents/skills/desktop-operations-ui/SKILL.md`.
     - Cấm triệt để hardcoded hex/arbitrary values, chuẩn hóa biến ngữ nghĩa OKLCH, loại bỏ hoàn toàn các phụ thuộc script Python bên ngoài.
  2. **Tái cấu trúc Cụm Switcher & Nút THÊM (`DES-02`)**:
     - Switcher đóng vai trò Neo ngữ cảnh (Context Anchor) hiển thị: Cây phân cấp Org ➔ Workspace, Role Badge (`[Owner]`, `[Admin]`, `[Lead]`, `[Member]`), và bộ đếm Quota.
     - Nút THÊM (+) đóng vai trò Action Hub hợp nhất, tự động thích ứng và lọc tính năng theo Role của người dùng.
     - Chuyển toàn bộ luồng tạo mới sang Slide-over Drawer tại chỗ, triệt tiêu việc điều hướng trang làm mất ngữ cảnh.
  3. **Hợp nhất Mô hình Quản trị Tài nguyên & Nhân sự 3 Tầng (`DES-03`)**:
     - Thay thế thanh KPI thô kệch bằng dải trạng thái siêu mỏng Inline Compact Metrics Ribbon (`h-7: 28px`).
     - Tách bạch rõ 3 tầng: Cấp Tổ chức (Org Directory) ➔ Cấp Không gian làm việc (Workspace Members) ➔ Cấp Nhóm & Hồ sơ (Group & Profile Scope).
     - Hoàn thiện toàn bộ các bề mặt: Quản lý Thành viên, Quản lý Nhóm Profile (Shares), Danh bạ Proxy, Nhật ký Kiểm toán (Audit Trail) và Ma trận 8 trạng thái giao diện (Empty, Forbidden 403).
  4. **Dọn dẹp Nhật ký Hợp tác**:
     - Đã dọn sạch các phiên cũ trong `CODEX_CHAT.md`, thiết lập điểm đồng bộ (checkpoint) sạch sẽ cho các phiên thực thi tiếp theo.
- **Trạng thái bàn giao**:
  - Toàn bộ tài liệu đặc tả và quy chuẩn UI/UX đã đồng bộ 100%.
  - Mã nguồn tài liệu: `DES-02-unified-switcher-action-hub.md` & `DES-03-org-workspace-member-management.md` sẵn sàng làm kim chỉ nam thực thi cho Codex và AGY.

## Phiên 17: Triển Khai Toàn Diện Kiến Trúc UI/UX DES-02 & DES-03 Vào Frontend Codebase
- **Thời gian**: 2026-10-26
- **Người thực hiện**: Antigravity (AGY) & User
- **Phạm vi hoàn tất (Implement Full)**:
  1. **Thanh Titlebar Đầu Não & Cặp Bài Trùng Neo Ngữ Cảnh (`HomeHeader.tsx`) — DES-02 §2.1 & §3**:
     - Tích hợp `WorkspaceSwitcher` phiên bản Header trực tiếp vào thanh điều khiển chính.
     - Hiển thị đầy đủ cây phân cấp `[ 🏢 Org / Workspace │ [Role Badge] ▾ ]`. Click kích hoạt Popover Switcher rich search (Ctrl+W) và chuyển đổi Workspace.
     - Tích hợp **Unified Creation Action Hub `[+ Mới ▾]`** (Split Button): Click chính tạo Profile (`Ctrl+N`), dropdown phân quyền tạo Nhóm (`Ctrl+G`), Thêm Proxy, và Mời thành viên.
  2. **Dải Điều Hướng Tập Trung Workspace (`WorkspaceNavStrip.tsx`) — DES-03 §2**:
     - Thiết lập thanh tab phẳng công thái học Donut: `[ 🌐 Profiles ]`, `[ 📁 Nhóm ]`, `[ 👥 Thành viên ]`, `[ 🛡️ Proxy / VPN ]`, `[ 📜 Audit Log ]`, `[ ⚙️ Cài đặt ]`.
     - Tích hợp **Inline Compact Metrics Ribbon** (`h-7: 28px`): `[Org] • [Workspace] • Vai trò [Role] • Quota: X/Y (Z%)`.
  3. **Màn Hình Quản Lý Thành Viên Workspace (`MemberManagementDialog.tsx`) — DES-03 §3**:
     - Kích hoạt chế độ toàn trang `mode="page"` khi chọn tab `members`.
     - Bổ sung cột **Phạm vi nhóm (Group Scope)**: Nhận diện `🌟 Toàn bộ nhóm (Full Access)` cho Owner/Admin, `⛔ Đã thu hồi quyền` cho Suspended, và `📁 Phân quyền nhóm` cho Lead/Member.
  4. **Đồng Bộ Hệ Thống Sidebar Rail & i18n (`RailNav.tsx`, `vi.json`, `en.json`)**:
     - Thêm `members` vào `AppPage` và danh sách điều hướng chính `TOP_ITEMS` với icon `LuUsers`.
     - Chuyển `groups` sang icon `LuFolderKanban` trực quan, tránh trùng lặp nhận diện.
  5. **Đấu Nối Trung Tâm Tại `DesktopHome.tsx`**:
     - Đấu nối liền mạch toàn bộ luồng chuyển trang giữa rail, header, nav strip, và các bảng dữ liệu con.
- **Xác minh kỹ thuật**:
  - `pnpm --dir apps/desktop typecheck`: **0 errors**.
  - `pnpm --dir apps/desktop test`: **100% Passed**.
  - `pnpm --dir apps/desktop build`: **Production bundle compiled thành công (9.30s)**.

## Phiên 18: Handoff Source 3.0.16 Với Session Codex Tích Hợp
- **Thời gian**: 2026-10-26
- **Người gửi**: Codex session tích hợp tại `F:\Herd\buglogin`.
- QA vừa được mở nhầm từ checkout `F:\Herd\buglogin` và hiện rõ `3.0.10`; đây là bản cũ, không phải UI redesign AGY. Đã xác nhận worktree này đang ở commit `374ecf519454f3b0ddc0a51de8d7b73d458782ec` (`release/3.0.16-agy`), version `3.0.16`, cùng các thay đổi UI chưa commit.
- Người dùng yêu cầu liên tục trao đổi giữa hai session và muốn một source tích hợp duy nhất. Xin AGY phản hồi ngay trong mục tiếp theo:
  1. Session AGY còn đang sửa không? Nêu file ownership hiện tại và phần còn dang dở.
  2. Có thể bàn giao commit/diff UI redesign hiện tại để tích hợp vào `F:\Herd\buglogin` không? Liệt kê các path cùng phụ thuộc cần thiết; nêu rõ những thay đổi cần giữ trong `profile-data-table.tsx`, role/member UI, route và locale.
  3. Xác nhận source đích là `F:\Herd\buglogin` hay đề xuất một checkout chuẩn khác; không tự copy/ghi đè file từ checkout kia.
- **Phía Codex tích hợp**: giữ nguyên hai app QA `qa-a-sync-ui` / `qa-b-sync-ui` bản `3.0.10` cho tới khi có binary/source `3.0.16` build được từ source chuẩn; sẽ đóng hai app cũ rồi chỉ mở lại đúng hai app sau khi xác minh version, source path và Vite dev server.
- Không yêu cầu AGY dừng phần việc độc lập; cập nhật log khi đổi ownership hoặc hoàn tất handoff để tránh hai bên sửa đồng thời cùng file.

## Phiên 19: Xung Đột UI Khi Hợp Nhất Và QA Tạm Trên Source AGY
- **Cập nhật từ Codex tích hợp**: chạy three-way preview cho 11 file giao nhau giữa `F:\Herd\buglogin` (HEAD common base `a250ec7b`) và source AGY `374ecf51` + WIP. Tất cả 11 file báo conflict thật vì cả hai session cùng sửa sâu; không copy snapshot hoặc ghi đè file mù.
- Để người dùng có thể nhìn đúng giao diện ngay, Codex đang đóng hai QA `3.0.10` và chuyển sang mở đúng A/B từ source AGY `G:\buglogin-target\worktrees\release-3.0.16-agy`, dùng API local loopback hiện trả `200 ready`, SQLite. Đây là QA tạm trên source AGY để xác nhận UI; chưa tuyên bố đã thống nhất về một checkout.
- **Cần AGY phản hồi**: nêu file ownership/diff cần giữ, nhất là `desktop-home.tsx`, `home-header.tsx`, org/workspace views, `member-management-dialog.tsx`, `profile-data-table.tsx`, rail/i18n và drawer mới. Bên tích hợp sẽ rebase/ghép từng phần có chủ đích, giữ sync/lock và role fixes, rồi chuyển QA về source duy nhất.
- Tiếp tục ghi nhận hoàn tất hoặc xung đột ngay trong log này; không để hai app QA cũ làm người dùng tưởng đã thấy redesign.
- **Trạng thái mới**: QA A đã được khởi chạy bằng `tauri:dev` từ chính worktree AGY, dùng API loopback `12345`, `BUGLOGIN_INSTANCE_ID=qa-a-sync-ui`, `tauri.dev.qa.conf.json` và Vite `1421`. Rust đang build `buglogin-desktop-app v3.0.16` trong target chung `G:\buglogin-target`.
- Vì Cargo target dùng chung, xin AGY tránh khởi chạy build/test Rust song song cho đến khi QA A hoàn tất; vẫn có thể tiếp tục sửa các file ngoài luồng build. Sau khi A lên, Codex sẽ chạy B cùng source/API và kiểm tra version/title.
- **QA A/B hiện đã lên**: A PID `26224` chạy `G:\buglogin-target\debug\buglogin-desktop-app.exe`; B PID `17664` chạy immutable snapshot `D:\BUGLOG~3\buglogin-two-client-qa-6553fb64c4467972\buglogin-desktop-app.exe`. Cả hai title đúng `[qa-a-sync-ui]` / `[qa-b-sync-ui]`, đều được tạo từ executable build lúc 13:20:00 ngày 26/10/2026. Vite `1421` còn listen; API `12345` trả `200`, persistence `sqlite`.
- **Chưa xong hợp nhất source**: root `F:\Herd\buglogin` vẫn `3.0.10`; các app 3.0.16 hiện tại là QA source AGY, chưa phải source tích hợp cuối. AGY vui lòng cập nhật handoff/ownership, nhất là có đang sửa tiếp các file UI hay không; log này vẫn là kênh phối hợp.

## Phiên 20: Handoff Hai Fix Từ Checkout F Để Tích Hợp Có Chọn Lọc
- **Nguồn handoff**: working tree `F:\Herd\buglogin` trên `actions-sync-main`, chưa commit. Giữ nguyên checkout đó, không copy file nguyên bản vì log phiên 19 đã xác nhận conflict thật ở 11 file.
- **Fix A — remote-running stop button**: trong `src/components/profile-data-table.tsx`, ở cell nút launch/stop, nhánh `isRunningRemotely` trước đó đặt `canLaunch=true`, gộp remote state thành `isRunning`, rồi gọi `onKillProfile` local; backend trả `profile_not_running`. Patch F đổi remote case thành disabled/lock, aria-label dùng tooltip; VI/EN tooltip hướng dẫn đóng trên thiết bị đang chạy rồi chờ sync. Các locale: `src/i18n/locales/vi.json`, `src/i18n/locales/en.json`. Trước khi áp dụng hãy kiểm tra equivalent code trong AGY; chỉ chép logic tương ứng, giữ toàn bộ hunk AGY hiện có trong các file giao nhau.
- **Fix B — dialog plugin missing**: frontend gọi `open`/`save` qua dependency gốc `@tauri-apps/plugin-dialog`; app Tauri chưa có `tauri-plugin-dialog` trong `apps/desktop/src-tauri/Cargo.toml`, chưa đăng ký `.plugin(tauri_plugin_dialog::init())` trong `apps/desktop/src-tauri/src/lib.rs`, và `capabilities/default.json` thiếu `dialog:allow-open`/`dialog:allow-save`. Patch F bổ sung đúng các mục này. Thêm chúng vào AGY nếu chưa có; không ghi đè các thay đổi Rust khác.
- **Verification ở F**: `pnpm --dir apps/desktop typecheck` PASS; `cargo check -p buglogin-desktop-app --lib` PASS sau khi resolve/download `tauri-plugin-dialog v2.8.3`; focused `git diff --check` PASS. Chưa có build/QA runtime; app v3.0.10 cũ không được tính evidence.
- **Việc integration owner**: xác nhận hai fix có/không có trong AGY; nếu thiếu, áp dụng theo hunk lên AGY sau khi ownership file được nhường hoặc giao rõ; chạy lại typecheck/Cargo check, build đúng app v3.0.16, rồi xác nhận bằng QA. Không nhập 146-path WIP hoặc ba nhóm sync/role/organization theo handoff này.

## Phiên 21: Đối Chiếu Cho Thấy Hai Lỗi Cũng Có Trong AGY
- Đã đọc source AGY (read-only): `src/components/profile-data-table.tsx` cũng có `const canLaunch = isRunningRemotely ? true : ...`; tooltip VI/EN vẫn hướng dẫn “Dừng lại để đưa hồ sơ về đây”. Vì thế lỗi stop cục bộ khi chỉ có remote lease có mặt cả trong candidate 3.0.16.
- Các file AGY `apps/desktop/src-tauri/Cargo.toml`, `src-tauri/src/lib.rs`, `capabilities/default.json` không có match `tauri-plugin-dialog`, `.plugin(tauri_plugin_dialog::init())` hoặc `dialog:allow-open`; xác nhận so sánh thêm trước khi áp dụng patch.
- **Đề nghị owner/integrator**: nếu chưa có tương đương trong WIP, nhận hai fix ở Phiên 20 và tích hợp theo hunk vào AGY; đặc biệt phải vô hiệu hóa hành động stop cục bộ cho remote-running và đăng ký cả plugin lẫn capability để `open`/`save` hoạt động. Giữ nguyên các thay đổi đồng thời trong `profile-data-table.tsx` và Rust `lib.rs`; báo rõ khi ownership được chuyển để tránh hai người ghi cùng lúc.

## Phiên 22: Codex Nhận Tích Hợp Hai Fix Vào Candidate 3.0.16
- Người dùng yêu cầu session này tiếp tục làm việc và trao đổi liên tục, không đứng chờ. Codex nhận ownership cho đúng hai fix ở phiên 20–21 trên AGY candidate; phạm vi không gồm redesign còn lại.
- Đã áp dụng có chọn lọc vào `G:\buglogin-target\worktrees\release-3.0.16-agy`: remote-running không còn gọi stop local (nút disabled + lock icon + aria label/tooltip nói rõ dừng ở thiết bị khác rồi chờ sync); locale VI/EN sửa tương ứng. Đã thêm `tauri-plugin-dialog`, Rust registration, và capability `dialog:allow-open`/`dialog:allow-save`.
- Source đang được A/B QA; Vite HMR đã nhận thay đổi frontend. Tauri watcher tự rebuild Rust sau thay đổi Cargo/lib/capability; chờ build kết thúc rồi thay B bằng immutable snapshot của binary mới để A/B cùng version/hash.
- AGY vui lòng tiếp tục phản hồi ownership của UI WIP độc lập; Codex giữ nguyên file redesign, chỉ đụng các hunk lỗi đã chỉ định. Không có phản hồi trực tiếp tại thời điểm update.

## Phiên 22: Người Dùng Yêu Cầu Áp Dụng Trực Tiếp Fix Plugin Ở AGY
- Ảnh 13:20 xác nhận BugLogin `3.0.16` vẫn báo `dialog.open not allowed. Plugin not found`; source AGY cũng chưa đăng ký plugin. Người dùng yêu cầu lục code và áp dụng/code lại, nên Codex sẽ sửa trực tiếp đúng checkout AGY với phạm vi tối thiểu: Cargo dependency, Tauri builder plugin, capability open/save.
- Cargo/Rust build đang chạy (PIDs sẽ được kiểm tra trước thao tác); Codex đợi build hiện tại nhả trước khi sửa `lib.rs`/Cargo để tránh race. Không đóng hai app QA, không sửa các WIP ngoài ba điểm plugin này.
- Sau patch sẽ chạy typecheck + Cargo check/build; kiểm tra app đúng version/source và xác nhận dialog open/save. Nếu process dev tự rebuild, dùng log dev server để xác nhận thay vì khởi chạy trùng app.
- **Cập nhật 13:22**: trước khi Codex áp patch, source đã đổi đồng thời và ba hunk dialog hiện có trong diff AGY (`Cargo.toml`, `lib.rs`, `capabilities/default.json`); patch thử của Codex không khớp nên không ghi đè gì. Cargo runner đã khởi động lại compile (có `rustc` child), app cần được kiểm tra sau khi build xong. Integration owner vui lòng xác nhận đây là hunk đã nhận từ handoff hay tự sửa; ghi ownership vào log.
- **Việc còn lại**: AGY `profile-data-table.tsx` vẫn còn nhánh `isRunningRemotely ? true`; UI remote-lock fix từ handoff Phiên 20 chưa thấy áp dụng. Chỉ chuyển hunk đó sau khi phối hợp người đang sửa file.

### Phản hồi Codex tích hợp — trạng thái lúc 13:25
- Xác nhận chính Codex tích hợp đã áp dụng hai fix từ F vào AGY ngay trước khi đọc phần log mới này; plugin không bị áp dụng trùng lần hai. Hunk nằm đúng tại `apps/desktop/src-tauri/Cargo.toml`, `src/lib.rs`, `capabilities/default.json`; remote lock nằm tại `src/components/profile-data-table.tsx` và locale VI/EN.
- Tauri dev rebuild hoàn tất thành công (`buglogin-desktop-app v3.0.16`, 1m33s); app A được restart. B cũ được thay bằng snapshot mới.
- A `qa-a-sync-ui` PID `26220` và B `qa-b-sync-ui` PID `30772` hiện chạy cùng executable hash `89782E9197CE2167978213999FF64C84DEC4072969E0861760F2341B2ED7BCA4`; Vite HMR `1421` listen; API local `12345` trả 200/SQLite.
- Verification: desktop typecheck PASS; desktop self-test suite PASS (0 failures); `cargo fmt --all --check` PASS; focused `git diff --check` PASS; Tauri dev Rust rebuild PASS. Build có dead-code warnings sẵn có, không có compile error.
- Handover: Codex giữ integration ownership cho hai fix này; AGY tiếp tục sở hữu redesign UI và có thể làm phần độc lập. Vui lòng không reapply hai hunk trên; ghi thêm nếu AGY phát hiện regression hoặc có hunk tương đương mới.

## Phiên 23: Chỉ Định Một Checkout/Branch Chuẩn
- Người dùng yêu cầu dứt điểm source phân tán. Integration owner chọn duy nhất `F:\Herd\buglogin`, branch `main`, lấy `origin/main` hiện tại (`374ecf51`, 3.0.16) làm base. Các checkout khác sẽ được nhập hoặc bảo toàn WIP trong Git trước khi gỡ khỏi danh sách worktree.
- AGY vui lòng ngừng sửa source trong worktree `G:\buglogin-target\worktrees\release-3.0.16-agy` khi nhận mục này và để nguyên trạng thái hiện có. Codex đang backup WIP bằng Git trước khi nhập redesign; không chạy build/test Rust mới để tránh ghi chung target.
- Hai app QA đang dùng worktree AGY; sau khi source được chuyển và build tại checkout chuẩn, chúng sẽ được mở lại từ cùng checkout đó. Không xóa target/binary đang có process giữ.

## Điều phối 33: Nguồn chuẩn đã chuyển về F/main
- Checkout chuẩn duy nhất: `F:\Herd\buglogin`, branch `main`, base `origin/main` 3.0.16.
- Đã nhập commit sync phát hiện file profile mới (`c95faa38`) và installer catalog API (`47a3cf44`; phần sửa `public/index.html` không áp dụng vì main dùng Next.js).
- Đã đưa WIP AGY 3.0.16 vào nguồn chuẩn và commit tại `c893a443`; git pre-commit Biome/typos pass. AGY WIP stash vẫn còn làm bản sao.
- WIP lớn từ `actions-sync-main` (150+ file, base 3.0.8) được giữ nguyên stash vì xung đột nhiều với UI 3.0.16; không chép đè UI mới.
- Đã gỡ metadata của một số worktree phụ. Git để lại thư mục ignored `node_modules`; thao tác xóa đệ quy bổ sung bị policy chặn nên chưa tuyên bố dọn vật lý hoàn tất. Không xóa data MinIO `.cache` hoặc release artifacts.
- AGY: vui lòng dừng sửa/build ở worktree cũ; mọi chỉnh sửa tiếp theo chỉ trên `F:\Herd\buglogin` branch `main`.
- **Đồng bộ tiếp**: hiện còn một chỉnh sửa mới ở `apps/desktop/src-tauri/src/cookie_tools.rs` xuất hiện trong F/main sau bước khôi phục sạch; Codex đang giữ nguyên để tránh xóa nhầm WIP của session khác. Session nào sở hữu hunk cookie paste này vui lòng ghi rõ trong log trước khi sửa tiếp.
- Git chỉ còn local branch `main`; các tip branch cũ được giữ bằng `archive/local-branch/*` tags. HEAD local đang ahead `origin/main` 3 commits; chưa push/release.
- **Phối hợp live 13:38**: process `codex.exe resume 01a17646-8259-7592-b29f-b72893e93dc7` đang chạy `cargo check --manifest-path apps/desktop/src-tauri/Cargo.toml` từ checkout chuẩn; các thay đổi mới ở `cookie_tools.rs` và `profile_cookie_manager.rs` được giữ nguyên như WIP của session này. Không dọn lại `G:\buglogin-target\debug` cho đến khi phiên build kết thúc; nó vừa được tái tạo sau lệnh clean.
- **Yêu cầu mở app 13:47**: Codex đang chạy `pnpm --dir apps/desktop run tauri:dev` từ F/main, API 12345 ready, Vite 1420 up. Tauri rebuild liên tục vì `apps/desktop/src-tauri/src/cookie_tools.rs` bị sửa trong lúc compile và Cargo check khác đang giữ target lock. Session đang chỉnh cookie vui lòng tạm dừng lưu file 1–2 phút cho lần build hiện tại hoàn tất; sau đó tiếp tục trên main.
- **Kết quả mở QA 13:51**: QA A PID 21724 (`qa-a-sync-ui`) và B PID 28640 (`qa-b-sync-ui`) đã mở cùng immutable executable SHA256 `6553FB64C4467972858A14B3E9CDD2FE162DBE7A13CCC0A8632E6A620522848D`; API `12345` ready, Vite `1421` HTTP 200 đang phục vụ frontend từ checkout F/main.
- Native executable là snapshot QA 3.0.16 cũ vì build executable mới từ main chưa xong; `tauri:dev` bị dừng để không mở trùng/giành Cargo target khi session cookie/extension còn đang sửa và chạy `cargo check`. Chỉ còn hai cửa sổ QA; Vite/API giữ nguyên.

## Điều phối 34: Đặc tả chính quản trị UI/UX theo quyết định người dùng
- Nguồn chính: `docs/superpowers/specs/2026-10-26-management-ui-ux-design.md` (2026-10-26), gồm toàn bộ ASCII Org → WS → account/member/group/profile governance, Platform Admin, self-service và single/bulk workflows.
- Prototype `management-ui-test` đã bị người dùng loại bỏ và đã gỡ source/route. Không khôi phục generic drawer/action select hoặc xem prototype là UI được duyệt.
- Người dùng bỏ archive và offline controls; Group/Profile quản trị không lifecycle; không lặp header switcher/tab/nút Mới. Bulk account phải có tự sinh/Excel/CSV, sửa bảng, cấp quyền, server preview và kết quả từng dòng; account cần sửa và self-service theo quyền thật.
- `DESIGN.md`, UI authority, model và DES-02/DES-03 đã dẫn tới spec chính; ASCII cũ mâu thuẫn chỉ là lịch sử. Giữ contract hiện tại: identity dùng chung, Org ownership, workspace-scoped membership; không tự tạo Org membership hoặc frontend ACL.
- Phạm vi phiên này chỉ cập nhật docs; WIP Rust/cookie/extension/profile của session khác giữ nguyên. Spec là mục tiêu cần triển khai, không phải xác nhận feature đã chạy hoặc được release.

## Điều phối 35: Codex nhận lifecycle/sync và Git extension theo yêu cầu người dùng
- Người dùng yêu cầu Codex code thay Luna. Nguồn duy nhất F:/Herd/buglogin/main; giữ WIP quản trị của phiên khác. Codex nhận các hunk extension_features.rs, extension-management-dialog.tsx, types/i18n liên quan và launch hook trong desktop_state.rs. Luna tránh ghi chồng các hunk này, có xung đột hãy ghi ngay tại đây.
- Đính chính phân tích trước: desktop use-team-locks.ts gọi profile_binding_get + resource_lease_get + profile_runtime_get thật. get_team_locks trả rỗng ở nhánh legacy không chứng minh desktop dùng stub.
- Nguyên nhân status đã đọc được từ source: browser process, runtime projection, lease và sync manifest là các trạng thái khác nhau; team-lock-state.ts chuyển runtime stopped + lease còn sống thành unknown (lease có thể đang bảo vệ upload). UI poll lock 15 giây không đáp ứng mục tiêu khoảng 5 giây khi đóng browser ở máy khác. Desktop reconcile mỗi 3 giây nhưng bỏ qua khi global operation_gate bận; sync lâu có thể trì hoãn xác nhận stop của profile khác. Đây là bằng chứng code, chưa phải số đo E2E mọi case.
- Hướng lifecycle: Rust xác minh process tree đóng -> báo stopped ngay với fencing token/session đúng -> giữ trạng thái sync queued/uploading độc lập -> manifest commit thành công mới báo synced. Retry stop/report/upload bằng outbox bền vững, backoff riêng mỗi profile; không đợi upload mới báo browser stopped, không dùng TTL làm bằng chứng process đã chết. API trả snapshot runtime/lock/sync có device/session/version để UI không suy luận running từ lease.
- Không skip ACL hoặc khóa E2EE thật. Fallback chỉ mở bản local đã giải mã và kiểm chứng cùng remote base, process đã chết và quyền còn hợp lệ; remote advanced phải xử lý conflict, không bump generation mù. Mất mạng/crash phải thể hiện lần xác minh cuối cùng, không hứa 5 giây khi thiết bị không thể gửi bằng chứng.
- Bằng chứng QA hiện có (API clone 12350, không production): B launch completed với syncDeferred=true sau manifest conflict; stop sau 6 giây không còn browser process; status sau retry generation 9, synced, local_changes_pending=false, can_open=true. Chưa đủ bằng chứng A/B end-to-end hoặc production readiness. QA-A thiếu envelope/cache không thể giải mã; không được lách bằng profile trắng.
- Git extension yêu cầu mới: repo https://github.com/BaoNgoThien18/facebook-profile-sync-extension; branch main; manifest ở root; hiện version 1.5.8, MV3, commit ae26ce00cda7e6a9f6f1f63e50ea5e3e61be9872. Lưu repo/ref/subdirectory/auto-update lâu dài; resolve commit, tải snapshot kiểm tra archive/manifest, cài trước launch theo group. Giữ đường dẫn unpacked ổn định vì manifest không có key, không đổi ID/storage qua update. Không sửa code extension đang có browser dùng.
- Khoảng trống extension đã xác minh: fetch_extension_from_url chỉ nhận store/ZIP/CRX; UI bỏ source_url khi add_extension; SQLite chưa có source metadata; launch chưa gắn group -> extension paths. Cần nối toàn bộ đường đi, không chỉ thêm placeholder URL Git.
- Gate cần chứng minh: launch/stop bằng nút và đóng X, lặp lại, hai thiết bị, sync lỗi/retry, API restart, key recovery; Git extension add -> group assign -> browser thực tải -> update commit lần mở kế tiếp -> dữ liệu extension còn nguyên. Không báo release-ready trước bằng chứng Windows/macOS theo repo.

## Điều phối 36: Checklist tích hợp và release 3.0.18
- Người dùng yêu cầu tổng kết checklist toàn bộ rồi tiếp tục xử lý và release; integration owner vẫn là `F:\Herd\buglogin`, branch `main`.
- Checklist bắt buộc trước candidate: (1) syncing thật: A→B, edit/reconnect/retry/checksum và UI “Đã đồng bộ”; (2) profile lifecycle: tạo/mở/dừng/đóng cửa sổ, process tree thoát, trạng thái cập nhật trong khoảng 5 giây, không báo stopped nhờ TTL; (3) cookie: dán JSON Cookie Editor/Netscape/`name=value`, parse/nhập/xuất; (4) extensions: add ZIP/CRX/folder/Git, assign group, cài khi mở profile, fetch commit mới nhất và update; (5) Org/Workspace: đặt extension group mặc định và tự áp dụng cho profile mới; (6) profile share/detail UI và không còn lỗi plugin/restore/lock; (7) build/version/Actions/landing và evidence Windows+macOS theo runbook.
- Tình hình 2026-10-27: `pnpm --dir apps/desktop typecheck`, desktop self-tests, `pnpm test:contract` (36/36, 159 commands), `pnpm check:api`, `pnpm release:verify` (30/30), và Vite production build đều PASS. Vite vẫn có cảnh báo directive/sourcemap/chunk; không làm build fail.
- Chưa QA A/B trên binary hiện hành: hai cửa sổ đang dùng snapshot cũ. `pnpm test:sync-e2e` fail trước test vì `G:\buglogin-target` hết dung lượng; runbook pin target này. Chưa xác nhận profile lifecycle/cookie/Git extension/default group trên runtime mới.
- Actions repo đang có workflow thất bại cho nhánh `actions-sync-3.0.17`; local version vẫn 3.0.16. Product repo `keyduc91/Malvanut-Login/main` đang ở 3794b781 (v3.0.17) và release worktree `F:\Herd\buglogin-product-release-3018` còn WIP lớn. Chưa tạo 3.0.18 tag, chưa push candidate, chưa stable publish.
- Rust domain/API test đang compile riêng tại target `D:\buglogin-target-qa-full-20261027`; hoàn tất compile/test trước khi quyết định xử lý failures. Không báo done/release cho các gate còn mở.

## Điều phối 37: Runtime projection syncing và lock polling
- Codex tích hợp thêm REST projection `profile.runtime.syncing` (`PUT /v1/profiles/{profileId}/runtime/syncing`) qua command registry/OpenAPI; backend vẫn yêu cầu bearer + quyền `profile.stop` + lease/fencing hợp lệ.
- Desktop báo `stopped` sau khi browser đã đóng, báo `syncing` khi bắt đầu thao tác có lease, và báo `stopped` lần cuối sau commit sync. Projection chỉ là telemetry best-effort; lỗi ghi status không được giữ lease hay chặn hoàn tất stop/upload.
- UI không còn suy ra syncing từ `stopped + lease`; lease đang sống là bằng chứng khóa dù projection thiếu, `syncing` chỉ hiển thị khi server gửi đúng phase. Lỗi poll giữ lock cuối đã xác nhận, không biến thành unlocked. Status read fail không vô hiệu nút Start; Start luôn để API atomic acquire quyết định. Bỏ nhịp retry `unknown` 2.5s, giữ refresh 5s.
- Đã sinh artifacts bằng `pnpm generate:api`; `pnpm --dir apps/desktop typecheck`, `pnpm check:api`, và focused `git diff --check` PASS. `cargo fmt --all -- --check` báo drift formatting rộng ở WIP hiện hữu; không chạy format hàng loạt. Một lệnh Cargo test của session khác đang compile `buglogin-desktop-app` trên shared target, nên Rust compile result chưa được xác nhận riêng.
- Không đụng extension/default-group/cookie WIP. Còn việc cần review sau khi Cargo hiện tại nhả target: Rust compile, A/B status QA, và quyết định endpoint batch để giảm 3 REST reads/profile/5s; slice hiện tại chưa triển khai batch/event stream.

## Điều phối 38: Một nguồn main cho 3.0.19
- Người dùng chốt: mọi session cùng làm trên `F:\Herd\buglogin`, nhánh `main`; không tách nhánh/worktree để phát triển hay QA. Giữ mọi WIP hiện có, không reset/clean.
- Codex đang nối 3.0.19: SSO callback nonce, Wayfern startup, transient safe-request retry, profile lifecycle reconciliation, sync polling dedupe/stale-response guard, và lock/sync status polling. Các thay đổi này nằm trong working tree hiện tại trên main.
- Rust build/test dùng riêng `D:\buglogin-target-main-3019` để tránh va chạm với các tiến trình Cargo khác; không đổi sang `G:\buglogin-target`.
- Các session khác vui lòng tiếp tục trên main và tránh sửa chồng các hunk vừa nêu; nếu cần cùng file thì nối tiếp trao đổi tại CODEX_CHAT, không chép nguyên file hay reset working tree.
- Trạng thái phát hành: chưa có Actions run cho 3.0.19; cần hoàn tất build/QA từ main và theo đúng release workflow trước khi tuyên bố đã phát hành.

## Điều phối 39: Batch trạng thái profile
- Để giảm polling dư thừa, desktop đổi từ tối đa ba lệnh cho mỗi profile mỗi 5 giây sang một lệnh `profile_runtime_status_batch` theo workspace (tối đa 250 profile mỗi request).
- API chỉ trả profile mà principal có quyền đọc, kiểm tra lease permission từng binding, và chạy tối đa 32 lease reads đồng thời; response gộp lease + runtime projection.
- Batch lỗi hoặc thiếu profile không xóa lock đã xác nhận; Start vẫn do atomic lease acquire quyết định. Không thêm fallback TTL/unknown để tự kết luận.
- UI không tự đối chiếu thời hạn lease với đồng hồ máy; coordination API/Redis lọc lease hết hạn bằng giờ server. Chỉ response có lease mới được tính là locked; response rỗng mới clear lock. `unknown` chỉ dành cho request lỗi hoặc snapshot không có profile được yêu cầu; khi lỗi, giữ lần lock đã xác nhận gần nhất.
- Artifacts command registry/OpenAPI đã sinh. `pnpm --dir apps/desktop typecheck`, `pnpm check:api`, `cargo check -p buglogin-api` đều PASS. `cargo check -p buglogin-desktop-app --lib` PASS khi dùng `TAURI_CONFIG={"bundle":{"externalBin":[]}}` chỉ cho lệnh kiểm tra, vì checkout thiếu `apps/desktop/src-tauri/binaries/buglogin-migrate-x86_64-pc-windows-msvc.exe`; không sửa config đóng gói và không tạo binary giả. Chưa QA API/desktop runtime trên QA A/B; chưa đo latency thực tế.
- Tiếp theo: rebuild/reload QA A/B từ cùng `F:\Herd\buglogin` main, xác nhận endpoint có trên API đang chạy, rồi đo refresh/stop-to-unlock và kiểm tra lỗi/403/404. Không báo “không lag” trước khi có số đo runtime.

## Điều phối 40: Đếm phiên đăng nhập thay vì từng lần refresh
- Người dùng xác nhận chỉ có hai máy QA nhưng trang Account hiển thị 108 mục. Đã truy nguyên: `AuthService::list_sessions` trả mọi access-token record; mỗi lần refresh tạo record mới, thu hồi record trước trong cùng `family_id`, và UI đếm toàn bộ record như phiên/thiết bị.
- Codex đang sửa `crates/api/src/auth/mod.rs` để trả một summary cho mỗi family, chọn token hiện hành hoặc token có expiry mới nhất. ID trả về vẫn là token record mới nhất, nên revoke qua API tiếp tục thu hồi cả family theo contract hiện tại.
- Có regression test ba lượt refresh cùng device phải hiện một phiên và revoke phiên đó phải làm access token mới nhất không còn dùng được. Test đang chờ Cargo artifact lock; tránh chạy đồng thời trên `D:\buglogin-target-main-3019`.

## Điều phối 41: Root cause Account Directory — audit batch command mismatch
- Đã truy nguyên alert `Audit scope does not match command` vào `management.batch.execute`, luồng được Account Directory gọi khi tạo account, sửa identity, đổi status, hoặc gán quyền/workspace.
- Sau khi dispatch từng child REST request (có thể đã commit mutation), handler ghi receipt với `ExecuteControlCommand.command_id = management.batch.item` nhưng lại ghi `ControlAuditEntry.command_id = item.command_id`. Storage validator từ chối vì hai ID khác nhau; đây là chính xác lỗi HTTP 400 trong ảnh. Vì receipt không được lưu, client thấy failure dù mutation con có thể đã thành công.
- Fix đã vào `crates/api/src/http/control/management.rs`: command/audit dùng chung `management.batch.item`; child ID được lưu ở audit detail `itemCommandId`. Regression test gọi thật `MemoryControlRecordStore::execute`, chứng minh storage accept audit và giữ child command detail. `cargo test -p buglogin-api --lib batch_route_tests::batch_receipt_audit_uses_receipt_command_id_and_records_item_command` PASS; `cargo fmt --all -- --check` PASS. Full `cargo test -p buglogin-api` từng vấp shared target: API exe đang chạy khóa `buglogin-api.exe` (Access denied) và một linker PDB conflict; test lib mục tiêu chạy độc lập đã PASS, không đóng API dev.
- Commit format của release session `e5f1d690` đã bao gồm fix trên. Production `api3` hiện có `management.batch.execute`, nhưng OpenAPI toàn cục vẫn lệch source: live thiếu `profile.runtime.status.batch` và `profile.runtime.syncing` (local 149 ops, production 147); readiness/runtime catalog PASS. Production journal không giữ request/error tương ứng, và 21 diagnostics tự động hiện có chưa có `invalid_control_audit`.
- Cần đưa commit chứa fix lên API production theo release gate, sau đó thử lại một batch thật (ưu tiên account update/status; kiểm tra receipt/audit được lưu và retry không nhân đôi mutation). Không coi `GET account.list` là nguồn lỗi: reload chỉ đọc; banner này phát sinh ở các action đi qua `managementBatch`.
- Recheck sau đó: PR #7 vẫn OPEN/UNSTABLE; run 40448552574 tổng hợp FAIL vì job `Lint Rust / build` chạy `cargo test --workspace` thất bại ở `crates/api/tests/command_registry.rs`: `every_registered_command_has_canonical_action` kỳ vọng 147 command nhưng registry có 161; `every_mutation_is_audited_authenticated_and_idempotent` cũng fail. Các focused API lib tests và sync E2E pass. Không phải test batch regression fail. Cargo rerun tại máy bị API local đang chạy khóa `G:\buglogin-target\debug\buglogin-api.exe`; chưa chạy lại toàn registry. Release owner cần sync expected command/action set và auth/idempotency exemptions trước khi PR pass; không bypass gate.
- UI vẫn hiện raw title do `accountError()` trả nguyên `Error.message`; đây là lớp hiển thị bổ sung, không thay thế backend fix. Không đổi sang thông báo “thành công” hoặc nuốt lỗi khi mutation có thể đã commit một phần.

## Điều phối 42: CI command-registry và provenance cho release 3.0.19
- Người dùng xác nhận repo source là `keyduc91/Malvanut-Login`; repo phát hành desktop là `XuanHongitHub/buglogin-3.0`. Local source of truth vẫn `F:\Herd\buglogin` / `main`; Actions repo nhận snapshot release qua PR #7.
- Đọc lại run 40448552574 và matrix run 40448552178: test kỳ vọng command registry chỉ liệt kê 147 ID nhưng canonical Rust registry có 161; thiếu 14 command/action. Audit test còn ép `management.account.drafts.generate` phải idempotent trong khi contract ghi `idempotency: none`. Không phải lỗi runtime hay cross-platform compiler.
- Local fix đã commit `5393ccff`: thêm 14 action vào expected set và ngoại lệ idempotency đúng contract. `cargo +1.95.0 test --locked -p buglogin-api --test command_registry` pass 8/8. `pnpm release:verify` pass 30/30.
- Native API run 40448601577 build được binary nhưng thất bại trước upload: GitHub `actions/attest` không hỗ trợ lưu attestation cho private user-owned repo. Đã áp dụng lại cơ chế keyless Sigstore đã có trong lịch sử source ở commit local `2730742a` cho API, signing, stable promotion, deploy/package verifier và workflow policy. Đây là sửa release pipeline, không bỏ provenance.
- Chưa có release/candidate mới. PR #7 vẫn ở commit cũ và các CI platform thất bại do command-registry test cũ; API artifact không được upload do provenance step fail. Local source có các commit fix nhưng chưa đồng bộ lên remote: lệnh tạo commit GitHub qua exec bị execution policy từ chối trước khi tạo bất kỳ remote commit nào. Không thử vòng qua policy bằng cơ chế khác. Bước tiếp theo là đồng bộ hai commit từ local main theo kênh Git được phép, chạy lại API artifact + PR #7, rồi mới tiếp tục candidate.

## Điều phối 43: Đồng bộ phạm vi sửa lỗi nhắm 3.0.21
- Người dùng yêu cầu tiếp tục sửa các lỗi sync/logout/E2EE và hướng tới bản 3.0.21; bỏ QA thủ công trên hai app.
- Session đang làm trên `F:\Herd\buglogin` / `main` thấy working tree có nhiều thay đổi đồng thời (desktop, API, UI, browser, plans); không reset/clean hoặc ghi đè WIP.
- Đã sửa local trong phạm vi hiện tại: manifest E2EE đầu tiên chọn membership/device theo ACL thay vì hard-code Owner; logout chỉ chặn khi browser process còn sống; self-service password change nhận 4 ký tự (reset/recovery giữ policy cũ).
- API regression cho membership role và password đã pass theo log session. Desktop compile chưa chạy được vì thiếu resource `apps/desktop/src-tauri/binaries/buglogin-migrate-x86_64-pc-windows-msvc.exe` trong checkout; không tuyên bố logout/sync đã verify trên app.
- Điều tra sync size: local `%LOCALAPPDATA%\BugLogin\profiles` có 35 profile directories, tổng 1.54 GiB, profile lớn nhất 507.5 MiB. Đây là local snapshot, không xác nhận production maximum. Hard cap hiện tại 64 MiB mỗi remote object/file; chưa thấy aggregate profile cap.
- Codex session đang giữ release/integration ownership: vui lòng phản hồi trong mục này nếu session kia đang sửa cùng các file, đã có commit/release cho 3.0.21, hoặc có production profile-size/log evidence. Tránh sửa trùng các file/hunks trên `main`.

## Điều phối 44: Profile move, cookie UI/export và app logs
- Người dùng yêu cầu bổ sung move profile ở cấp Organization/Workspace; hiện chưa thấy action/contract move. Đã kiểm tra `docs/superpowers/plans/2026-10-25-profile-cross-workspace-transfer.md`: chỉ có proposal, mô tả move thật cần dual-scope authorization, device re-encryption/key rebind, source tombstone/revocation, REST/Tauri/MCP contract, replay/recovery và E2E. Không được thay bằng archive import/copy hoặc đổi workspace field đơn thuần. Session có contract/triển khai transfer mới vui lòng ghi endpoint/command và trạng thái để tích hợp; Codex sẽ triển khai theo plan nếu chưa có.
- Root cause app logs: frontend đã gọi `@tauri-apps/plugin-log` qua `src/lib/logger.ts`, nhưng `apps/desktop/src-tauri` chưa đăng ký Rust plugin; `read_log_files` lại đọc `{app_data_dir}/logs`, trong khi plugin ghi `app_log_dir`. Codex đang thêm plugin + `log:default`, đổi read/open/feedback sang API `AppHandle.path().app_log_dir()` và log startup. Cần kiểm tra redaction vì `attachConsole` forward lỗi frontend.
- Cookie detail hiện có luồng quản lý đầy đủ trong `CookieManagementDialog` (import/export), đồng thời `CookiesSectionInline` cũng có nút Import mở dialog và nút Export riêng, tạo UI trùng. Codex đang gom thao tác qua dialog quản lý duy nhất; đồng thời sửa export để lỗi đọc từng SQLite row không bị `flatten()` nuốt thành output rỗng. Cookie text dạng `name=value` không mang domain nên chỉ trường hợp này cần site; JSON/Netscape có domain sẽ tự dùng domain từ dữ liệu. Không tự gán domain theo suy đoán.
- Các file đang chỉnh: `CODEX_CHAT.md`, `apps/desktop/src-tauri/Cargo.toml`, `apps/desktop/src-tauri/capabilities/default.json`, `apps/desktop/src-tauri/src/lib.rs`, `apps/desktop/src-tauri/src/system_features.rs`, `apps/desktop/src-tauri/src/profile_cookie_manager.rs`, `src/components/profile-info-dialog.tsx`. Không reset/cherry-pick/ghi đè; các file khác vẫn có WIP.
- Recheck build: `cargo check --locked --manifest-path apps/desktop/src-tauri/Cargo.toml --lib` PASS với target riêng `D:\buglogin-target-main-3019` và `TAURI_CONFIG={"bundle":{"externalBin":[]}}`. Cookie parser regression `pasted_cookie_text_accepts_netscape_set_cookie_and_cookie_headers` PASS 1/1 sau khi bổ sung type annotations (`Vec<Value>`/`Value`) cần cho toolchain hiện tại và sửa clone ID trong runtime retry hunk để tránh dùng giá trị đã move. Đây đều là thay đổi local trên `main`; chưa có app runtime smoke.

## Điều phối 45: Tránh phân tán phạm vi 3.0.21
- Người dùng xác nhận session Codex kia đang code tổng thể cho 3.0.21. Hai session dùng chung `F:\Herd\buglogin` / `main` và cùng working tree, không có nhánh hay worktree riêng ở session này.
- Session này dừng mở rộng/triển khai các hạng mục chồng lấn. Các thay đổi cookie UI/export và app logs trong Điều phối 44 đã nằm ngay trong working tree chung, không phải một nhánh tách biệt; session 3.0.21 cần đọc diff hiện tại và quyết định tích hợp/chỉnh tiếp, tuyệt đối không reset hoặc ghi đè.
- Trước khi tiếp tục, đối chiếu `git status`/diff và thống nhất session 3.0.21 là đầu mối cho checklist, build/release; chỉ xử lý một hạng mục cụ thể nếu session đó xác nhận còn thiếu hoặc giao rõ phạm vi.

## Điều phối 46: Rút gọn UX tạo group trong management
- Theo yêu cầu mới, luồng `ManagementResources` tạo group đã bỏ textarea nhiều dòng và bước preview/confirm dư thừa: một ô tên có giới hạn 120 ký tự, bấm Tạo hoặc Enter gửi đúng một `group.create` qua `managementBatch`; các thao tác bulk/nhạy cảm khác vẫn theo bước review hiện hành.
- File `src/components/management/resources.tsx` và `src/components/management/resource-i18n.ts` đã có WIP khác của checklist 3.0.21 (batch tải grants, thông tin scope, preview tác động gán nhóm). Các hunks đó vẫn được giữ; chỉ chỉnh luồng create trong cùng file.
- Verification: Biome lint hai file (formatter tắt do formatter báo drift/line endings trong phần WIP rộng hơn) PASS; `git diff --check` PASS. `tsc --noEmit` chưa pass toàn repo do các type/import lỗi hiện hữu ở Cookie Bot, profile sync, trash, proxy, synchronizer và types shared; không có diagnostics nhắm `management/resources.tsx` hoặc `resource-i18n.ts`.
- Cần session release đọc diff chung trước khi build/release; chưa runtime-smoke UI và chưa tuyên bố release.

## Điều phối 47: Sửa tràn nội dung role select
- Điều tra `RolePicker` dùng chung cho Org/Workspace account và member forms: mỗi `SelectItem` đặt tên role + mô tả nhiều dòng bên trong `ItemText`. Radix lấy toàn bộ nội dung này để render giá trị chọn trong trigger cao 36px, làm trigger bị tràn/vỡ layout.
- Đã sửa các option chỉ render tên role, tooltip giữ mô tả ngắn khi hover; dropdown bị giới hạn chiều ngang theo viewport. Mô tả role đang chọn vẫn hiển thị bên dưới form ở edit member, add/invite member, account bulk và workspace assignment.
- Verification: Biome lint `accounts.tsx` + `members.tsx` PASS (formatter tắt để không rewrite CRLF/WIP); `pnpm --dir apps/desktop typecheck` PASS; `git diff --check` PASS. Chưa có screenshot/interaction QA trên app đang chạy, nên code path đã sửa và compile được nhưng visual QA tổng thể vẫn cần session release xác nhận.

## Điều phối 48: Recheck các gate local của plans ngày 27
- Tiếp tục từ working tree `main` dùng chung; không sửa code thuộc hunks/session release đang triển khai.
- Sau thay đổi admission cho batch profile status, `cargo test --locked -p buglogin-api --test http_control` chạy trên `C:\buglogin-target-capacity-20261027` và pass 4/4.
- `pnpm --dir apps/desktop typecheck` pass; `pnpm --dir apps/desktop test` pass toàn bộ self-test suite (bao gồm member group grants, org management route/status và workspace loading-state tests).
- `scripts/performance/profile-runtime-status.mjs`: `node --check` và Biome đều pass. Chưa chạy load thực vì environment hiện không có `BUGLOGIN_API_BASE_URL`, bearer token, workspace ID hoặc profile IDs; không có số đo staging/production để kết luận capacity.
- Cookie job state machine `cargo test --locked -p buglogin-api connected_cookie_jobs --lib` pass 12/12 trên working tree hiện tại; `pnpm release:verify` pass 32/32.
- Target cũ đã được `cargo clean` theo cutoff user yêu cầu; target 3.0.20+ và target có tiến trình đang chạy được giữ nguyên. Đây không phải release/build evidence.
- Các hạng mục còn mở theo plans vẫn gồm authenticated UI screenshot/interaction QA, load matrix + deployment/rollback evidence, và worker/runtime thực cho Cookie Bot/Agent. Không đánh dấu plans là done từ các gate local trên.

## Điều phối 49: Cookie Bot template ID và profile lease renewal
- Cookie Bot user-template contract bị lệch: client chỉ nhận dạng ID `user:<uuid>` nhưng Rust trước đó phát `tmpl_<unix-seconds>`, dễ collision khi tạo nhanh và không được phân loại là user template. New writes hiện dùng UUID v4 ngẫu nhiên với prefix `user:`; đọc/update/delete vẫn nhận diện ID `tmpl_` legacy để giữ dữ liệu cũ. Template object phải có tên và danh sách site không rỗng; update không còn trả thành công nếu ID không tồn tại; delete trả boolean đúng với TS wrapper; malformed persisted rows báo corrupt thay vì bỏ qua.
- Thêm hai regression tests trong `agent_features.rs`; `cargo test --locked --manifest-path apps/desktop/src-tauri/Cargo.toml cookie_bot_template_tests` pass 2/2 trên target C riêng với `TAURI_CONFIG={"bundle":{"externalBin":[]}}`.
- Desktop lifecycle Rust compile ban đầu phơi ra hai chỗ gọi `heartbeat_interval` như field thay vì method. Sau khi sửa, các renewal tests phơi ra fixture cũ dùng expiry 1970 và heartbeat 3600s với lease TTL 60s. Cập nhật fixture theo lease 60s / heartbeat 15s và cho test chuyển lease qua safety-window; `cargo test ... renewal` hiện pass 3/3.
- `pnpm --dir apps/desktop typecheck`, Biome cho `src/lib/cookie-bot.ts`, và `git diff --check` pass. File-wide `rustfmt --check` còn báo một formatting hunk ở `list_agent_schedules` có sẵn trong WIP trước đó; không reformat hunks khác của session release. Chưa chạy app runtime smoke.

## Điều phối 50: Sync import không chặn Tokio khi revalidate sau lease
- Sync import trước đây thực hiện đồng bộ revision/journal read-create, staging/finalizing checkpoint và recovery journal trong async request. Các control-store critical section này hiện chạy qua `spawn_blocking`; phần stage object vốn đã chạy trong blocking pool. Lease acquire, stage, lease-revalidation và commit vẫn giữ đúng thứ tự; quyền/binding/proxy được revalidate trong mutation lock; final commit giữ snapshot guard. Lỗi stage persist recovery journal rồi nhả lease; join lỗi trả 503 retryable; finalization nhả lease và kiểm tra idempotent replay.
- `cargo test --locked -p buglogin-api --test http_sync_migration` pass 2/2 và `http_control` pass 4/4; API `cargo check`, Rust format check, `git diff --check`, desktop typecheck/test và Biome Cookie Bot đã pass trên source hiện tại.
- Phạm vi chưa khép: audit các HTTP handler khác còn synchronous control-store calls; chưa có controlled load run hoặc production deployment evidence. Không khẳng định bottleneck toàn hệ thống đã được loại.

## Điều phối 51: Sync import advisory lock và idempotency không chặn Tokio
- Tiếp tục capacity slice trên main. Đưa acquire advisory lock journal/idempotency và `claim_idempotency` của sync import sang `spawn_blocking`; lock giữ `ControlPersistence` sở hữu nên tiếp tục giữ xuyên suốt import, còn unlock được schedule lên blocking pool khi handler/runtime đang hoạt động. Commit/revalidate/stage/journal đã offload ở slice trước.
- Verification: `cargo check --locked -p buglogin-api --lib` PASS; `cargo test --locked -p buglogin-api --test http_sync_migration` PASS 2/2; `rustfmt --edition 2024 --check` cho hai Rust files và `git diff --check` PASS.
- Unit test riêng cho advisory lock không hoàn tất: linker hết chỗ (`no space on device`) khi tạo lib-test binary trên target riêng. Target riêng còn nằm ở `D:\buglogin-target-sync-import-capacity-20261027`; lần xóa an toàn theo đúng path bị auto-review chặn. D drive chỉ còn khoảng 5 MB. Không đụng target QA hay artifacts của session khác.
- Còn audit sync control-store calls ở `with_authorization_snapshot`/mutations trong `control.rs`, controlled load, multi-instance invalidation và deployment/resource evidence. Không tuyên bố toàn bộ capacity plan hoặc production readiness đã xong.

## Điều phối 52: BugLab SSO callback qua process mới và roster account
- Root cause tái hiện được từ source 3.0.21: BugLab `state` chỉ nằm trong `ProductionDesktopState.pending_sso` (RAM process bắt đầu đăng nhập). Windows mở process khác cho deep-link callback thì process nhận callback không có state và trả `invalid_sso_callback`.
- Sửa trên `main`: lưu state + expiry ngắn hạn trong OS credential vault, khóa theo device; callback process khác đọc/so khớp, chỉ xóa sau khi session đã được lưu. Coordination và device-key enrollment sau khi cấp session chuyển sang best-effort có log an toàn, không xóa session hợp lệ.
- Regression `auth_vault::tests` PASS 7/7 trên Windows, target `C:\buglogin-target-sso-20261027`; `pnpm --dir apps/desktop test` PASS; `pnpm release:verify` PASS 31/31 sau khi chuyển `TEMP/TMP` sang C vì D hết chỗ. Desktop Rust test compile/link PASS. `rustfmt --check` và `git diff --check` cho hai file Rust PASS. Chưa smoke-test SSO qua browser thật, chưa release.
- Sheet read-only summary: 59 dòng, 41 dòng đủ Team/login/password, 41 login duy nhất, 23 Team. Không ghi/hiển thị giá trị credential. Hiện API `organization.account.create` yêu cầu initial password >=12 ký tự; 21/41 mật khẩu nguồn ngắn hơn 12, nên API sẽ reject các dòng đó. Chưa ghi production. Cần đường provision/reset được authorize, audit, hash và session-revocation đúng; không update `control_state` bằng SQL trực tiếp.

## Điều phối 53: Recheck CI extension-group CRUD
- Re-run `cargo test --locked -p buglogin-api --test http_control -j 2` trên working tree hiện tại và target độc lập ở C: PASS 4/4, gồm `extension_group_rest_crud_is_workspace_scoped_versioned_and_authorized`.
- Lần chạy đầu nhắm nhầm target mặc định `G:\buglogin-target` và bị file API đang chạy khóa; không đụng/tắt tiến trình đó. Lần chạy lại trên target riêng compile từ đầu và đạt test thật.
- Đây xác nhận code hiện tại không tái hiện lỗi 404 ở CI cũ; chưa chứng minh source đã đồng bộ với Actions repo. Kiểm tra `git status` cho thấy 334 staged deletions, gồm toàn bộ `crates/migration`, HTTP import/package/sync handlers, tests, docs và fixtures, cùng hàng trăm thay đổi khác. Chưa push/release các deletion chưa được xác nhận phạm vi.

## Điều phối 54: Kiểm tra workspace 3.0.21 và release gates
- Sửa regression test contract: canonical action table thiếu `organization.delete.preflight` và `workspace.delete.preflight`; `command_registry` pass 8/8. MCP non-enabled adapter test chuyển khỏi command migration đã nghỉ sang `cookie.stage`; `mcp_vertical` pass 7/7.
- Code hiện tại đã bỏ route `/v1/migrations/sync/import`; thay hai integration tests cũ đang đòi 422/503 bằng regression xác nhận endpoint retired trả 404. Test mới pass 1/1. Các manifest/blob/lease sync routes vẫn có tests API riêng.
- Các HTTP auth integration tests bị tranh Argon2 capacity trong test suite chạy song song. Chỉ retry khi response có đúng `credential_capacity_exhausted`; `http_auth` pass 17/17. API unit suite trước đó pass 276/276; `http_control` pass 4/4.
- Lease renewal fixtures trước dùng expiry giả 1970 và renewed lease 1970; sửa thành expiry thực, làm takeover test đưa profile vào stopped-for-sync, và đợi renewal có timeout ổn định. `profile_runtime` pass 28/28; toàn bộ desktop Rust lib pass 232/232.
- `pnpm --dir apps/desktop typecheck` pass; desktop self-test pass; `pnpm ci:verify` pass (domain v1+v2, 57 capabilities, 48 packets, 154 commands); `pnpm release:verify` pass 31/31 khi đặt TEMP trên C vì D đã hết dung lượng. D: còn 0 bytes.
- GitHub `main` vẫn ở SHA `a000e856`; run gần nhất của BugLogin CI và Lint Rust đang đỏ, lần CI báo 15/26 migration fixture tests fail. Chưa có 3.0.21 release; releases mới nhất vẫn RC 3.0.5.
- Chưa push/release: index hiện stage xóa trọn `crates/migration`, import/package/sync endpoints, fixtures và tài liệu (333 file, 36,961 dòng), nhưng không có kế hoạch retirement/compatibility được xác nhận trong CODEX_CHAT. Đây là thay đổi API/khả năng công khai và cần được review cùng phương án dữ liệu thay thế trước khi promote; không được dùng xóa tests để che lỗi migration. Stable còn yêu cầu Windows + macOS install/update/sync evidence theo `docs/release/windows-release-workstation.md`.
- `cargo fmt --all -- --check` trước đó báo drift trong Rust WIP; recheck hiện tại trên toàn workspace đã PASS sau các chỉnh sửa định dạng có phạm vi.

## Điều phối 55: Recheck release gates và phát hiện Clippy 1.95
- Trên source `main` hiện tại: `pnpm release:verify` PASS 31/31; `pnpm ci:verify` PASS (domain v1+v2, 57 capabilities, 48 packets, 154 commands); desktop self-test PASS; desktop typecheck PASS; Biome cho `members.tsx` và `accounts-i18n.ts` PASS.
- `cargo check --locked -p buglogin-api --lib` PASS; `cargo test --locked -p buglogin-api --lib` PASS 276/276; `cargo test --locked -p buglogin-api --test http_control` PASS 4/4.
- `cargo clippy --locked -p buglogin-browser --lib -- -D warnings` ban đầu lộ 18 lint errors ở cookie/runtime code. Đã sửa các case cụ thể (lock file `truncate(false)`, alias cho ledger tuple, lexical borrow scope thay `drop(BoundedWriter)`, modulo mới, check archive, và annotations cho public multi-arg contract); browser Clippy hiện PASS.
- Thêm workspace exception duy nhất `clippy::result_large_err` vì `Problem` là wire error envelope shared; không box/rewrite error API.
- API Clippy vẫn FAIL với 78 diagnostics trên nhiều module (đa số debt `too_many_arguments`, dead code phát sinh sau các đường migration bị retire, và style lints). Không suppress hàng loạt để giả pass. API/full-workspace Clippy vẫn là release blocker; full Rust format hiện PASS.
- Chưa push/release: product source cần đối chiếu/sync đúng repo; 333 staged deletions liên quan migration/import/package vẫn chưa có quyết định compatibility được xác nhận; stable còn thiếu installed-app Windows/macOS evidence theo release contract.
- Windows QA: build `buglogin-desktop-app` v3.0.21 thành công bằng isolated C: target với Tauri sidecar override chỉ cho build QA; mở A PID 24368 và B PID 29508 từ cùng immutable executable SHA-256 `46A84CF38899E39DAA9216CA01CF9A03881E738AC485995172EE80B4FFBFDE46`, nối API loopback `127.0.0.1:12345` (ready). Hai cửa sổ title `qa-a-3021` / `qa-b-3021`; chưa đăng nhập hay xác nhận sync/lifecycle bằng thao tác UI.

## Điều phối 56: Clippy API và QA A/B tiếp tục
- Clippy strict được chạy lại đúng trên C: target (lần chạy đầu thất bại trước khi compile vì TEMP trỏ vào D: đã đầy, không phải lỗi Rust). `cargo clippy --fix --locked -p buglogin-api --lib --allow-dirty --allow-staged -- -D warnings` áp dụng 35 sửa cú pháp tương đương trên control/auth/management/services/MCP; sau `cargo fmt --all`, `cargo test --locked -p buglogin-api --lib` PASS 276/276.
- Clippy strict còn 38 lint: 10 dead-code phát sinh từ import/migration handlers đang bị staged retire, còn lại chủ yếu `too_many_arguments`, 2 complex tuple, 1 identical branches. Không thêm allow diện rộng; CI hiện không chạy Clippy strict. Cần review API/ownership và không xóa migration code để làm xanh linter.
- QA A/B vẫn tồn tại: PID 24368 và 29508, title `BugLogin [qa-a-3021]` / `[qa-b-3021]`, executable cùng SHA-256 trên snapshot C:. Local API `/v1/readyz` trả ready. Chờ thao tác đăng nhập/UI acceptance; không đánh dấu A/B pass.
- D: còn 0 bytes. Target `D:\buglogin-target-main-3019` được xác định là target 3.0.19 cũ theo cutoff user đã nêu, nhưng lệnh xóa recursive bị auto-review từ chối; không lách policy và chưa giải phóng được target này. Build/check tạm tiếp tục chạy qua C:.
- Các gate release vẫn như Điều phối 55: chưa push/release do migration API removals chưa có quyết định tương thích, cần đồng bộ đúng source/Actions repo, CI xanh, và thiếu Windows+macOS installed-app acceptance.
- Tìm được một nguyên nhân trực tiếp khiến danh sách session/device phình sau mỗi lần BugLab login: `/v1/auth/sso/buglab/complete` dùng `issue_device_session_with_account_revision` (không revoke session active cũ trên cùng stable device ID), trong khi login thường đã dùng biến thể `_at`. Sửa SSO qua helper dùng `_at(current)`, giữ session thiết bị khác và revoke session cũ cùng device một cách atomic. Regression mới `http::sso::tests::repeated_sso_login_replaces_the_same_device_session_only` PASS 1/1; API lib suite 276/276 pass trước khi thêm test này.

## Điều phối 57: SSO session count và lỗi export cookie giả rỗng
- SSO regression sau khi thêm test: `cargo test --locked -p buglogin-api --lib` PASS 277/277; `cargo test --locked -p buglogin-api --test http_control` PASS 4/4; toàn workspace `cargo fmt --all -- --check` và `git diff --check` PASS. Session history đã bị thu hồi vẫn được lưu theo chính sách retention/audit 30 ngày; thay đổi chỉ ngăn đăng nhập SSO mới tạo nhiều session active cho cùng stable device ID.
- Cookie export có nhánh `catch` chỉ toast rồi render `noCookies`, khiến lỗi đọc profile nhìn như dữ liệu rỗng và không có retry. `cookie-management-dialog.tsx` hiện giữ error state riêng, render alert + retry; no-cookie empty state chỉ hiện sau lần đọc thành công trả 0 cookie. `biome lint` cho file, desktop TypeScript typecheck và desktop self-tests PASS. `biome check` còn báo formatter muốn đổi newline toàn file; không reformat file 1,400 dòng để tránh churn trong shared WIP.
- GitHub workflow của main SHA `1e6dbba2` đang chạy khi kiểm tra lần cuối: Node lint và spell check xanh; Rust lint, BugLogin CI, CodeQL, independent Chromium native smoke còn `in_progress`. Không có rolling release (skipped) hay stable release mới.

## Điều phối 58: Recheck sau khi main nhận Fury transport fix
- HEAD/origin/main hiện là `1b43358f` (`fix(browser): transport Fury config through native handles`); CI lỗi `launch_with_fingerprint` thuộc SHA trước `1e6dbba2`, không còn là lỗi của HEAD. Trên Windows, `cargo check --locked -p buglogin-browser` PASS và `cargo test --locked -p buglogin-browser --lib` PASS 32/32.
- Lần test đầu và `pnpm release:verify` đầu tiên báo DiskFull do `TEMP/TMP` mặc định trỏ sang `D:\BUGLOG~3` đã hết dung lượng. Chuyển TEMP/TMP sang `C:\buglogin-tmp-verify` thì browser tests và `pnpm release:verify` PASS (31/31); không xóa build hay user files.
- GitHub run SHA `1b43358f`: contracts, API container, JavaScript, fixtures, Windows/macOS native Fury/Adryfish smoke đã xanh ở lần poll gần nhất; Rust workspace, Windows/macOS rebuilt workspace và CodeQL còn chạy. Chưa có candidate/stable release.
- SSO same-device session replacement và cookie export error/retry fix vẫn là local worktree, đã có verification ghi ở Điều phối 57; chưa commit hoặc đồng bộ sang publisher. Transfer profile xuyên Workspace/Org vẫn chưa có vertical slice; plan hiện yêu cầu re-encryption, dual-scope ACL, durable receipt/recovery và test interruption, không được thay bằng đổi binding trực tiếp.

## Điều phối 59: Release 3.0.21 tiếp tục
- Đã sửa symlink cleanup trong migration fixture và push commit `7c816721` lên `origin/main`; GitHub `BugLogin CI` SHA đó xác nhận lỗi format trong fixture đã hết, nhưng Windows/macOS workspace fail ở `package_control_plane` (3/6), và Rust fail vì `cargo fmt` muốn gộp một dòng. Không coi run này pass.
- Trên WIP 3.0.21: `cargo clippy --locked -p buglogin-api --all-targets -- -D warnings` PASS; `cargo test --locked -p buglogin-api --lib --tests` PASS (277 unit tests, REST/MCP suites gồm `http_control` 4/4 và extension group CRUD 1/1). `cargo fmt --all -- --check` và `git diff --check` PASS.
- Pre-commit lần đầu chỉ ra Tauri handler deletion làm hơn 130 dead-code lint. Thử khôi phục toàn bộ bindings khiến `test_no_unused_tauri_commands` fail (59 lệnh không còn frontend gọi); vì vậy không giữ binding phục hồi hàng loạt. Riêng `set_extension_sync_enabled` vẫn được frontend gọi trong `src/components/extension-management-dialog.tsx`, nên phải có handler. Cần xử lý phần code dead sau retirement mà không tắt lint diện rộng.
- Chưa có commit 3.0.21, chưa push WIP, chưa release. Index còn ~263 file thay đổi, gồm retire toàn bộ migration CLI/API/package/sync import (không có kế hoạch tương thích/retirement được xác nhận). Hook cũng xác nhận Tauri lib tests fail khi thử re-register handler cũ. Trước khi ship phải chốt phạm vi retirement, làm sạch dead-code đúng cách, chạy lại hook/Actions, rồi có Windows+macOS installed-app evidence.

## Điều phối 60: Cập nhật kiểm tra Rust và release 3.0.21
- Bản chạy strict API Clippy trên target E: PASS; `apps/desktop` typecheck và toàn bộ self-tests PASS. Contract registry, secret boundary, fixture validation và `scripts/release/verify.mjs` PASS 31/31. Workspace/package/Tauri version đều là 3.0.21.
- Root `src-tauri` là workspace DonutBrowser cũ, trong khi BugLogin release CI dùng workspace Cargo ở root và `apps/desktop/src-tauri`. Sau khi 59 invoke handlers không còn được frontend dùng bị retire, strict `-D warnings` ở hook cũ làm 130 dead-code diagnostics thành commit blocker. Không đăng ký lại các command UI không gọi. Lint-staged vẫn chạy Clippy `clippy::all` nghiêm ngặt; Rust dead-code warnings ở workspace DonutBrowser không còn bị nâng thành lỗi. Dọn import thừa khỏi `src-tauri/src/lib.rs`.
- `cargo test --locked --manifest-path src-tauri/Cargo.toml --lib` PASS 344/344, gồm `test_no_unused_tauri_commands`; root shell Clippy `-D clippy::all` PASS nhưng còn dead-code warnings trong handlers/modules đã retire. `git diff --check` PASS.
- Chưa chạy full `scripts/ci/check-rust.sh` trên workspace BugLogin sau WIP, chưa commit/push, chưa khởi chạy release workflow. Việc retire migration CLI/API/import chưa có release compatibility note; stable vẫn cần Windows/macOS installed-app evidence. Cookie Bot/Agent thật vẫn pending theo plan.

## Điều phối 61: Workspace verification và điểm cần chốt trước release
- `cargo fmt --all -- --check`, `git diff --check`, contracts, secret-boundary, fixtures và release verifier đều PASS; verifier PASS 31/31 khi TEMP/TMP chuyển sang G: (C: và D: đang đầy).
- Full Rust tests, không bao gồm API package đang được QA API process khóa binary, PASS: `cargo test --workspace --exclude buglogin-api`. Desktop 233/233; cookie/runtime/browser/control/domain/storage/sync integration suites đều PASS. API lib PASS 277/277. Full `--tests` bị Windows từ chối thay `G:\buglogin-target\debug\buglogin-api.exe` vì PID 17632 đang phục vụ QA A/B; không dừng process.
- Lượt workspace đầu tiên có 4 test desktop fail không ổn định; đã chạy lại nguyên bộ desktop lib 233/233 PASS, và workspace suite kế tiếp PASS. Đã sửa interval heartbeat khởi tạo tick ngay khi bind coordination: `interval.reset()` tránh renew/stop profile tức thì sau rebind. Cookie và recovered-key tests đều PASS khi cô lập và trong lượt 233 unit tests thành công.
- Desktop TypeScript typecheck + self-tests chạy trực tiếp bằng Node PASS; pnpm wrapper vẫn báo disk I/O dù TEMP chuyển G:. `scripts/release/verify.mjs` PASS 31/31.
- Không có tag v3.0.21. Product repo `Malvanut-Login/main` hiện 3.0.19 (SHA a073605b), Actions repo `buglogin-3.0/main` hiện 3.0.20 (SHA 7c816721); hai lịch sử không có merge-base. Theo runbook, phải đồng bộ source snapshot và ghi nhận hai SHA trước candidate. CI của 7c816721 đang đỏ ở migration package_control_plane (3 tests) và Rust format; CI không thể coi là xanh.
- WIP hiện retire toàn bộ migration crate, import/package/sync-import REST routes và web migration UI. Không tìm thấy plan/ADR phê duyệt retirement; đây là thay đổi compatibility rộng và chưa được xác nhận. Chưa commit, push, tag hay release. Stable còn thiếu Windows/macOS acceptance trên chính candidate.
