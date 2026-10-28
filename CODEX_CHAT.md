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

## Điều phối 62: Candidate 3.0.21 và API parity live
- Root workspace vẫn resolve `@tauri-apps/plugin-log` 2.8.0 trong khi desktop/Rust dùng 2.10.1; GitHub candidate trước fail version check trên cả Windows/macOS. Đã căn root package + lockfile lên 2.10.1, `pnpm release:verify` PASS 31/31 (temp trên G: do disk-full mặc định), `cargo tree --locked -p buglogin-desktop-app -i tauri-plugin-log` xác nhận 2.10.1.
- Commit `50fc7b94` đã push lên `XuanHongitHub/buglogin-3.0/main`. Candidate run `40594557514` validate PASS, hiện signing/build Windows+macOS đang chạy; chưa có candidate artifact hoàn tất, tag, stable feed hay landing update.
- Production API verifier vừa chạy read-only: ready + runtime catalog hợp lệ nhưng local source 125 paths/143 operations khác live 131/149. Live thiếu `organization.delete.preflight`, `workspace.delete.preflight`; live còn 8 operations migration (`migration.control.import`, `.rollback`, `migration.import`, `migration.inspect`, `migration.package.activate`, `.file.put`, `.verify`, `migration.sync.import`) đã bị source retire. Cần reconcile API compatibility/deployment trước stable; không đưa API release khi contract còn lệch.
- Malvanut `main` vẫn `a073605b` / 3.0.19, Actions `main` là `50fc7b94` / 3.0.21; hai lịch sử không có merge-base. Người dùng đã chốt release trên Actions repo; vẫn cần ghi nhận rõ snapshot/source mismatch theo runbook trước stable.
- Đã rà lại các hạng mục: BugLab callback qua process mới, same-device session replacement, cookie export error state, app-log path/plugin, group-create UX, role-select overflow, Org/Workspace management và Git extension group có code trong main. Profile cross-Workspace/Org transfer vẫn là proposal chưa implement; OpenSpec yêu cầu dual-scope authorization, durable fencing/receipt, re-encryption với key mới, recipient envelopes và recovery. Không thể coi đã hoàn tất bằng đổi binding hoặc copy ciphertext.
- Stable promotion chưa được chạy. Theo release contract còn thiếu Windows/macOS installed-app acceptance trên artifact chính xác và API/OpenAPI reconciliation; candidate được giữ private trong lúc hoàn tất checklist.

## Điều phối 63: API parity và CI keychain
- Đính chính trạng thái production sau Điều phối 62: API v3.0.21 đã được deploy qua helper có receipt `20261027T224945Z-50fc7b94afb203c977ca66d72de7dc88628fa950-f73740edae671c18`. `/v1/readyz` ready; runtime catalog available; live OpenAPI khớp source 125 paths / 143 operations, SHA `7B03AAAFA93E525A643730F95DE83D609FC7C4AB7BC873EFF36BE86AD87B7DFE`. Hai preflight routes hiện được router nhận và trả 401 khi chưa xác thực.
- Candidate run `40594557514` ở SHA `50fc7b94` đã thành công, tạo artifact Windows x64 và macOS arm64 đã ký/notarize. Chưa có stable promotion.
- CI macOS Intel workflow fail vì test `cookie_stage_returns_opaque_id_without_echoing_secret` cần NativeVault/keychain nhưng runner không có keychain mở khóa. Thêm keychain tạm được sinh bằng password ngẫu nhiên, đặt làm default chỉ trong job `macos-x64`, rồi khôi phục default và xóa ở bước `always()`. Không nới lỏng keychain app hay production.
- Release verification local: `scripts/release/verify.mjs` PASS 31/31 với TEMP/TMP trên G:. Candidate 3.0.21 và API v3.0.21 cùng SHA `50fc7b94`; landing page đọc stable feed động, chưa cần build riêng.
- Còn phải push thay đổi workflow, chờ CI trên SHA mới, chạy workflow Promote Stable với candidate run hợp lệ và xác minh stable feed + landing download. Chưa có installed-app acceptance bằng máy Windows/macOS sạch; sẽ báo rõ trạng thái thay vì suy diễn từ CI.
- Các hạng mục sản phẩm được yêu cầu trước đó không đồng nghĩa đã hoàn tất toàn bộ: transfer profile giữa org/workspace chưa implement; BugLab login mới có unit test forwarding, chưa test end-to-end với provider; tài khoản spreadsheet chưa import vì thiếu connector/export được phép. Không ghi plaintext mật khẩu vào chat/log.

## Điều phối 64: Stable 3.0.21 đã phát hành
- Push CI keychain fix: commit `428c268f3974d0fa788f1663c8bbe5e81bee891d` trên `XuanHongitHub/buglogin-3.0/main`. Workflow promotion `40597987640` SUCCESS, dùng candidate ký `40594557514` từ source `50fc7b94`; không rebuild/re-sign artifact.
- Stable manifest và landing API đã được đọc trực tiếp từ máy local: cả `https://mavelnut-login.sgp1.digitaloceanspaces.com/releases/desktop/v3/stable/latest.json`, `downloads.json` và `https://app3.buglogin.com/api/downloads/latest` trả HTTP 200, version/channel `3.0.21`, Windows/macOS URLs đều trỏ immutable artifacts ở candidate `40594557514`. Workflow báo Cloudflare challenge với runner khi kiểm tra landing, nhưng stable S3 readback của workflow và local landing verification đều pass.
- API 3.0.21 vẫn ready, runtime catalog available, OpenAPI live/local khớp 125 paths / 143 operations. Deploy receipt ở Điều phối 63.
- Workflow `BugLogin CI` (`40597976989`) và `Lint Rust` (`40597976893`) trên commit keychain fix `428c268f` vẫn in progress ở lần kiểm tra cuối; macOS Intel bước tạo/mở keychain tạm đã pass. Cần lấy kết luận cuối khi chạy xong.
- Google Sheets/Excel connection discovery trả về không có connected session. Chưa đọc/ghi account credentials hay thay dữ liệu prod. Đã yêu cầu kết nối Google Sheets hoặc tải CSV an toàn; tiếp tục các phần độc lập trong lúc chờ.
- Không xác nhận tất cả kế hoạch sản phẩm xong: cross-org/workspace profile transfer chưa implement; BugLab login chưa E2E trên packaged app; Cookie Bot/Agent worker thực vẫn đang unavailable theo plan; account-sheet import còn chờ nguồn dữ liệu truy cập được. Stable 3.0.21 đã ship, các sửa tiếp theo cần candidate/version mới và release mới.

## Điều phối 65: Native BugLab callback không bị rơi trước khi UI sẵn sàng
- Đính chính ghi chú cũ: BugLab SSO state đã được persist trong OS credential vault từ nhánh code 3.0.21; không còn chỉ giữ trong RAM.
- Native deep-link/single-instance handlers nay giữ callback `buglogin://` trong inbox có giới hạn 8 URL, loại URL ngoài scheme và dedupe, sau đó frontend đọc/drain inbox qua `desktop_deep_link_current`. Vẫn emit event cho UI đang mở; đường get-current/inbox bù cho callback đến trước listener.
- Verification local Windows: `cargo fmt --manifest-path apps/desktop/src-tauri/Cargo.toml -- --check`, hai test `single_instance_deep_link_tests`, `git diff --check` PASS.
- Chưa xác nhận callback packaged qua provider. QA A/B đặt `BUGLOGIN_INSTANCE_ID` có vault/device identity riêng và cố ý tắt single-instance guard; callback process OS mở không kế thừa instance ID, nên inbox đơn thuần chưa route callback về đúng instance A/B. Không đánh dấu vấn đề “callback mở app khác” đã xong và chưa phát hành bản sửa này.
- Stable 3.0.21 đã publish trước đó. Thay đổi inbox local chưa commit/push ở thời điểm ghi mục này.

## Điều phối 66: Rà soát an toàn roster trước khi provision
- Không có Codex Google Sheets session; link export CSV công khai tải được nên đã đọc metadata trực tiếp. Không in/ghi credential values vào chat hoặc log.
- CSV có 59 dòng; 41 dòng đủ Team + Mail Login mới + Password; 41 login không trùng; 22 Team khác nhau theo chữ thường; 18 dòng thiếu ít nhất một trường. Trong 41 dòng đủ, 25 login trông như email, 16 không đúng định dạng email; 20 password đạt API min 12 ký tự, 21 ngắn hơn.
- API organization account create yêu cầu email và password tối thiểu 12 ký tự. Không tự bịa email, nới policy mật khẩu, hay đổi password nguồn; production account write chưa thực hiện. Cần xử lý dữ liệu đầu vào theo API contract trước khi provision.

## Điều phối 67: CAS cho phiên đăng nhập cùng thiết bị
- `SessionPersistence` trước đây scan session cùng `device_id` rồi commit việc revoke + cấp phiên mà không có revision fence theo cặp account/device. Hai API instance có thể cùng đọc snapshot cũ và cùng để lại session active.
- Bổ sung `device_session_head` với record ID SHA-256 có domain separation trên account/device; head revision, session/refresh/family, account revision và các revoke/delete được commit trong cùng transaction.
- Khi CAS head thua race, reload/recompute revoke và retry tối đa 3 lần; account revision stale và conflict ở delete/device-code giữ nguyên lỗi gốc. Sau 3 race trả lỗi retryable `device_session_issue_conflict`.
- Regression head/revocation và login replacement same-device pass; `cargo test --locked -p buglogin-api --lib` PASS 279/279; `cargo fmt`/`git diff --check` pass. Đây là fix race có thể tái hiện ở kiến trúc multi-instance, chưa chứng minh toàn bộ lịch sử 108 session production do race này gây ra.
- Chưa commit/push hoặc release. Stable hiện là 3.0.21; session mới cần đồng bộ source/action repo và đi qua candidate gates trước khi promote stable.

## Điều phối 68: Regression race đăng nhập cùng thiết bị
- Bổ sung test chạy đồng thời hai lần cấp session cho cùng account/device trên cùng control store; cả hai request hoàn tất, durable head revision lên 2 và chỉ còn đúng một session family active.
- Test tập trung `concurrent_same_device_sign_ins_leave_only_one_active_family` PASS; thay đổi test và cập nhật plan là WIP trên `main`, chưa push riêng.
- Thử nghiệm 8 request đồng thời vượt giới hạn retry hữu hạn và có thể trả conflict retryable; gate giữ ở race hai API instance đúng kịch bản cần xử lý, không tăng retry không giới hạn.

## Điều phối 69: Sync lease và đo thời gian launch/sync
- Sync lifecycle: lease không còn hết hạn sau 5 phút nếu đang có lượt sync thật; 5 phút chỉ áp dụng cho trạng thái dừng chờ retry. Sync status chỉ được publish sau key/proxy/recipient preflight; nếu sync lỗi, publish `stopped`, giữ local dirty data, queue retry và nhả lease.
- Regression `active_sync_attempt_renews_past_idle_hold_then_releases_for_retry` PASS; chạy đủ `profile_runtime::tests` PASS 30/30; desktop typecheck/self-tests, Rust fmt/check và `git diff --check` PASS.
- App log nay đo từng bước launch preflight (extension, manifest, key rotation, pending proxy, device key) và tổng thời gian stop/sync với outcome `success`/`retry`, không log profile ID hay nội dung profile. Rust desktop `cargo check` PASS; production samples chỉ có sau khi cài build instrumented.
- Thay đổi đang là WIP trên `main`, chưa commit/push/release. Còn toàn bộ plan acceptance ngoài local (production timing/load matrix, multi-instance invalidation/resource policy, authenticated UI QA/OS evidence); Cookie Bot/Agent runtime thực và adapter security-current rebuild vẫn chưa hoàn tất.
## Điều phối 70: Dot sync không treo cam và retry nền nhanh hơn
- Nguyên nhân dot cam treo: bảng profile ưu tiên `runtime.activity=syncing` từ projection/lease server; nếu upload xong nhưng request cập nhật projection `stopped` thất bại, UI tiếp tục tô cam dù không còn lượt upload. Dot nay chỉ dựa trên event sync cục bộ/API sync status.
- Sync nền sau khi profile đóng vẫn chạy ngay; lỗi sẽ retry từ local data sau 3s, 6s, 12s... tối đa 30s giữa các lần (trước đây tối đa 5 phút). Lease được nhả giữa các retry và được gia hạn trong lúc upload thật đang hoạt động.
- Không có ETA tổng cố định: mỗi HTTP request timeout 60s; tổng sync tùy dung lượng/object/mạng. App log ghi duration_ms và success/retry sau mỗi lần; cần build chạy thực tế để có số production.
- `cargo fmt --check`, test backoff mục tiêu PASS, desktop typecheck PASS. Đang chuẩn bị candidate 3.0.24; chưa commit/push/release stable.

## Điều phối 71: Plan chống nghẽn API và sync — phiên lập kế hoạch
- Người dùng yêu cầu plan thật kỹ; phiên này chỉ tạo spec `docs/superpowers/specs/2026-10-28-sync-api-resilience-design.md` và plan `docs/superpowers/plans/2026-10-28-sync-api-resilience.md`, không sửa code ứng dụng hay deploy trong lượt lập plan.
- Main vừa tiến tới `dbf7b356`; forwarding `PostgresControlRecordStore::get_many` và `member_directory_views` offload đang được phiên khác bổ sung trong shared WIP. Không viết trùng; xác minh adapter thật sau khi source chốt.
- Release review quan trọng: `put_resource_blob` đang acquire shared advisory lock, rồi await `put_remote_chunk_async`, rồi mới explicit unlock. Nếu future bị hủy tại await, blocking task vẫn có thể tiếp tục nhưng nhánh unlock bị bỏ. Cần worker sở hữu trọn lock/work/cleanup và test cancellation; chỉ bọc S3 bằng spawn_blocking chưa đủ.
- Các điểm còn lại: `body.to_vec()` nhân đôi buffer lớn; permit/body accounting phải sống theo task thực kể cả disconnect; readiness còn gọi DB/S3 đồng bộ; GET snapshot guard đang giữ qua handler; retry presence/transport/whole-sync có thể chồng nhau.
- Hướng chốt trong plan: API an toàn trước và giữ route cũ; desktop bỏ GET check từng object, direct idempotent PUT, một retry budget + giới hạn upload toàn desktop. Batch resume/DB pool chỉ mở packet khi có số đo cần thiết. Không bỏ lease/ACL/checksum/GC fence để giảm tải.

## Điều phối 72: Khôi phục API và nới CPU runtime tạm — 2026-10-28
- Theo yêu cầu vận hành trực tiếp của người dùng, đã capture backlog `129/128` rồi restart riêng `buglogin3-api.service` lúc 02:41:49 UTC; PID mới `3354736`. Origin health/readiness và public health trả 200, backlog về 0. Không thay binary hoặc xóa dữ liệu.
- Theo yêu cầu nới tải tạm cho tới release tiếp theo, dùng `systemctl set-property --runtime buglogin3-api.service CPUQuota=100%`, tăng từ 50% lên tối đa một lõi trên VPS hai lõi. PID giữ nguyên, không restart thêm; `CPUQuotaPerSecUSec=1s`, health/readiness 200, backlog 0 ở lần kiểm tra sau thay đổi.
- Đây là override runtime; mất khi reboot nhưng có thể còn hiệu lực qua service restart/deploy. Release owner phải đối chiếu chính sách CPU đã version hóa và gỡ/khôi phục override khi deploy API sửa nghẽn. Nếu chính sách vẫn là 50%, lệnh rollback vận hành là `sudo systemctl set-property --runtime buglogin3-api.service CPUQuota=50%`; không tự đổi giới hạn RAM, TasksMax, NOFILE, DB/Redis hoặc dịch vụ BugIdea.
- Nới CPU chỉ giảm throttling, không sửa các đường sync I/O, cancellation, queue và retry trong plan 71. Không coi đây là bản sửa production hoàn tất.

## Điều phối 73: Việc chuyển sang release kế tiếp sau 3.0.30 — 2026-10-28
- Người dùng báo app 3.0.25, màn About bấm “Kiểm tra cập nhật” nhưng không thấy hoạt động. Candidate 3.0.30 đang được promote; không sửa/rebuild candidate hiện tại. Release sau phải điều tra và sửa luồng check-update, kiểm chứng trên bản cũ khi stable feed đã lên 3.0.30, rồi xác nhận UI hiển thị đúng phiên bản/cập nhật và tải đúng artifact ký.
- Tối ưu CI Rust: `.github/actions/setup-buglogin` đã bật cache pnpm; `rust-cache` hiện có ở rolling-release/sync-e2e/independent Chromium, nhưng thiếu ở `.github/workflows/signing.yml` (Windows/macOS) và `native-api-artifact.yml`. Thêm cache theo OS + target/toolchain + Cargo lock/config, dùng target dir `CARGO_TARGET_DIR` (Windows `G:\buglogin-target`), giữ cache tách theo nền tảng; không chia build binary hoặc key ký giữa OS. Work chỉ áp dụng sau khi bản 3.0.30 stable hoàn tất.

## Điều phối 74: Session 4 — lifecycle timeout/retry và xác minh nền tảng
- Đọc source `remote_api.rs`, `runtime_install.rs`, adapter runtime selection và regression tests. API client dùng connect timeout 10s/request timeout 60s; chỉ retry tối đa một lần cho request an toàn hoặc có idempotency key, với transport connect/timeout và HTTP 502/503/504. Retry-After được chặn tối đa 5s cộng jitter xác định. `send_bytes` không retry nội bộ, nhưng luôn gửi idempotency key và trả lỗi retryable cho owner sync. Chưa có bằng chứng source rằng cần nới timeout hoặc phát lại upload.
- Runtime target được lọc theo host OS/architecture, xác minh signed catalog, path confinement và checksum; profile pin version cụ thể và lỗi rõ nếu bản đó không cài. Không phát hiện sai OS/architecture trong code đã kiểm tra. Regression tests hiện có bao phủ version inventory/legacy layout/pinned selection và safe-vs-unsafe request retry.
- QA riêng còn thiếu: native install/update/launch trên Windows 10/11 x64 và macOS Apple Silicon; fingerprint theo OS/entitlement; KEK/legacy-envelope/key-vault migration với profile hiện hữu. Không sửa crypto/fingerprint/production data. Kết quả được ghi vào plan authoritative `2026-10-27-authoritative-profile-status.md`; đường dẫn ngày 28 trong giao việc không tồn tại.

## Điều phối 75: Session 1 — launch không tự đóng khi runtime projection lỗi
- Nhận việc trên main: phạm vi apps/desktop/src-tauri/src/desktop_state.rs và hồi quy cục bộ. Xác nhận launch hiện gọi stop_for_sync khi runtime_started thất bại sau khi browser đã spawn; đang phân loại lỗi xác thực/quyền/lease với lỗi retryable trước khi đổi hành vi.
- Phạm vi giữ nguyên, không sửa remote_api.rs hoặc profile_runtime.rs; bảo toàn WIP của session khác.

## Điều phối 76: Codex — session 2 projection hydrate/sync
- Nhận phạm vi `apps/desktop/src-tauri/src/profile_projection.rs` và test projection. File đang có WIP session khác về giới hạn bộ nhớ transfer/snapshot; giữ nguyên thay đổi đó, không stage/commit.
- Phát hiện hydrate xác minh generation cache đồng bộ trong async worker, rồi xác minh lại sau các lượt tải metadata. Chuyển cả hai lượt xác minh file/hash sang `tokio::task::spawn_blocking`, vẫn xác minh lần hai sau network wait để phát hiện local data thay đổi trong lúc chờ.
- Regression `profile_projection::tests::hydration_reuses_verified_generation_without_fetching_profile_blobs` PASS (1/1); `git diff --check` PASS. `cargo fmt --check` bị format lệch ở `desktop_state.rs` ngoài phạm vi, không sửa file đó.
- Rà soát commit recovery: marker ghi base/target generation; khi remote conflict, recovery giữ base local và không promote target cũ. Chưa có bằng chứng projection đang overwrite generation mới. Retry/điều phối sau stop nằm trong `desktop_state.rs`, do session 1 sở hữu; không thay logic ngoài phạm vi.

## Coordination 77: Session 3 - renewal identity fencing
- Scope: profile_runtime.rs and its regression test only; kept lease ownership and fencing checks intact.
- Fix: pending renewals now track fencing token and owner token per profile. A delayed completion clears only the matching renewal, so it cannot erase a newer lease's pending state.
- Added stale_renewal_completion_does_not_clear_current_lease_pending_state. cargo check --tests passes; focused cargo test is running in an isolated G:\buglogin-target-session3 because another session holds the shared test executable.
- profile_runtime::tests PASS: 36 passed, 0 failed.
- cargo check --tests, cargo fmt --check, and git diff --check PASS. The shared-target link failed because an existing test executable was open; isolated target test run succeeded. No lease/fencing checks were removed.

## Điều phối 78: Session 4 — API deadlines và runtime target theo host
- Scope được giao: `apps/desktop/src-tauri/src/remote_api.rs`, `apps/desktop/src-tauri/src/runtime_install.rs`, phần target selection trong `adapter.rs` nếu cần; không chạm desktop_state/profile_projection/profile_runtime, server, UI, identity policy hoặc crypto. Đọc ownership, lifecycle/signed-runtime spec và CODEX_CHAT trước khi sửa.
- Source issue đã sửa: generic API request dùng chung request timeout 60s nên lease/runtime projection/status read có thể chờ lâu; retry từng lần lại được phép timeout độc lập. Thêm timeout theo loại request: control/lifecycle 12s, metadata 30s, transfer/runtime catalog 60s; thêm deadline tổng 30s quanh generic retry loop. Retry vẫn tối đa một lần, chỉ safe/idempotent request; 401/403/404 nghiệp vụ không retry; Retry-After vẫn cap 5s + jitter. Blob `send_bytes` không có transport retry ngầm, giữ idempotency key và deadline transfer 60s để sync owner quyết định retry. Event stream cũng bị giới hạn 12s lúc mở kết nối.
- Runtime audit: host OS/arch xác định từ target Rust; inventory chỉ dò thư mục của host, xác minh signed catalog, path confinement, executable SHA-256 và install receipt. Cài theo version staging/rollback giữ bản tốt; profile version pin không fallback. Không tìm thấy sai target; không đổi `runtime_install.rs`/`adapter.rs`.
- Không chạy test/build/QA theo yêu cầu session. Chưa có xác nhận compile cho phần request deadline mới. Tích hợp cuối cần chạy desktop Rust tests/format và QA Win10/11 x64 + macOS Apple Silicon. Fingerprint OS/entitlement và KEK/legacy envelope cần QA riêng, không thuộc write scope; production data untouched. Không commit/push/release.
- Rà tiếp các đường HTTP trực tiếp: timeout tương ứng đã gắn vào send_text, runtime catalog bounded GET, desktop login, BugLab SSO start/complete, device token refresh, workspace event stream và profile list. Các đường này trước đó bypass `api_request_timeout` của generic send và dùng timeout chung 60s.
- Recheck policy: deadline-exceeded Problem giữ correlation của request; canonical API 401/403/404 bị ép `retryable=false` dù payload server khai báo ngược lại. Catalog bounded GET dùng metadata deadline 30s, blob transfer giữ 60s. Không thêm retries cho direct blob upload; Retry-After được trả về sync owner. Chưa compile theo chỉ thị không chạy build/test.
- Event stream giữ kết nối dài hạn: chỉ deadline 12s cho bước mở stream bằng `tokio::time::timeout`; không áp reqwest toàn-request timeout lên body SSE để tránh chủ động ngắt stream sau 12s.
- Runtime host mapping có lỗi đã chứng minh: mọi host OS ngoài Windows/macOS trước đó bị gán Linux qua nhánh `else`. `current_platform_architecture` nay nhận diện Linux tường minh và trả `runtime_platform_unsupported` cho host OS khác; kiến trúc vẫn chỉ cho x64/arm64. Chưa chạy compile/test theo lệnh session.
- Phòng thủ tại ranh giới installer: dù caller hiện lọc catalog theo host, `runtime_install::install` nay tự so platform/architecture của signed release với host trước mọi filesystem/download side effect; target chéo OS/arch trả `runtime_target_host_mismatch`. Không sửa caller ngoài scope.
- Chốt retry wall-clock budget: cả backoff transport và Retry-After/jitter đều được chờ bên trong thời gian còn lại của deadline tổng 30s. Nếu delay không vừa ngân sách, trả `api_request_deadline_exceeded` giữ correlation thay vì ngủ quá hạn rồi mới phát hiện.
- Deadline tổng bao cả phần đọc/parse response của `send` (cả body lỗi lẫn JSON thành công), không chỉ thời gian chờ headers. Slow/truncated body vì vậy cũng không kéo generic request vượt retry budget; đây không áp dụng cho SSE stream dài hạn hoặc blob `send_bytes` vốn có transfer timeout riêng.
- Audit riêng device token refresh: custom replay path trước đó retry mọi send error ngay lập tức và không có wall-clock budget. Nay chỉ retry connect/timeout hoặc server response lỗi retryable, tối đa hai attempts trong cùng 30s; delay transport 250ms, server 5xx tôn trọng Retry-After cap+jitter, mọi delay nằm trong deadline. Idempotency key ổn định được giữ nguyên. Chưa chạy tests/build như lệnh session.
- Session 1 đã sửa nhánh runtime_started: lỗi retryable giữ browser chạy và trả runtime projection pending kèm mã lỗi; lỗi phiên, quyền, lease/fencing và non-retryable vẫn đi qua nhánh dừng. Regression transient_runtime_projection_error_keeps_launched_profile_running PASS 1/1; desktop_state::tests PASS 27/27; cargo fmt và diff check PASS.
## Điều phối 79: Cập nhật full plan cho các session lifecycle
- Người dùng yêu cầu session 2 và 4 nhận full plan thay vì phần rút gọn; session 3 cần mở rộng task. Chưa chạy test/QA riêng từng session; kiểm tra tích hợp làm một lượt cuối.
- Session 1 tiếp tục full lifecycle trong desktop_state.rs: operation/lock wait, preflight, lease, spawn, runtime projection, cleanup, stale attempt và đo/log thời gian. Giữ regression hiện có, không ghi đè WIP chung.
- Session 2 full sync/projection: hydrate/cache/integrity, immutable snapshot, bounded transfer, commit generation fence, dirty recovery qua retry/restart, cancellation ownership và reopen không bị worker cũ chặn/ghi đè. Chỉ profile_projection.rs cùng test trong file; không sửa desktop_state/profile_runtime/remote_api/API.
- Session 3 mở rộng profile_runtime.rs: actor không bị network/blocking work giữ; launch/stop handoff tuần tự theo profile; heartbeat/renewal chạy độc lập; completion được fence theo owner token, fencing token và session epoch; transient renewal/status lỗi không tự dừng process khi authority còn hiệu lực; lease hết hạn/mất quyền thật vẫn được xử lý; stale stop/renewal không tác động session mới; process cleanup idempotent theo OS. Chỉ profile_runtime.rs và test trong file; không sửa desktop_state/profile_projection/remote_api.
- Session 4 full API/runtime client: deadline theo request, retry hữu hạn chỉ thao tác safe/idempotent, tổng retry budget, Retry-After, cancellation/permit ownership, error classification; runtime install/catalog verify signature/hash/path/receipt, đúng host OS+architecture, pinned version không fallback, atomic staging/rollback và lỗi thiếu/hỏng rõ. Chỉ remote_api.rs, runtime_install.rs, target selection adapter.rs nếu cần; không sửa desktop_state/profile_projection/profile_runtime/server/crypto.
- Cả bốn session code trong scope và cập nhật tiến độ/phát hiện ở mục Điều phối mới; chưa chạy test/build/QA, không commit/push/release. Tích hợp cuối chạy gate chung. Không xóa ACL/fencing/checksum, dữ liệu legacy, hoặc sửa fingerprint/KEK ngoài scope.

## Điều phối 80: Session 2 — integrity và fencing projection
- Scope ghi: `apps/desktop/src-tauri/src/profile_projection.rs` và test cùng file; `CODEX_CHAT.md` chỉ ghi tiến độ. Không sửa các file WIP ngoài scope.
- Nguyên nhân được chứng minh trong projection: snapshot ciphertext cho file thường dùng cache path ổn định theo workspace/resource/key/path; snapshot mới có thể ghi đè bytes mà một snapshot upload cũ còn tham chiếu. Cleanup ở cuối snapshot cũng xóa staged objects không thuộc snapshot mới, làm hỏng upload đang dùng cùng cache scope.
- Sửa cache object theo plaintext checksum để snapshot cùng path nhưng nội dung khác giữ staged ciphertext bất biến; bỏ cleanup tức thời vì projection không có bằng chứng upload cũ đã kết thúc. Stale sync commit bị chặn, dirty bytes được test giữ nguyên; stale hydrate bị fence trước tải và trước publish/cached return khi active-session marker đã tiến generation.
- Giữ manifest/schema, encrypted format, receipt, authenticated verification, active-session/commit recovery và permission/lease contract hiện hành. Thêm focused tests trong file cho immutable upload snapshot, stale commit/dirty preservation và stale hydrate fence.
- Chưa chạy test/build/QA theo yêu cầu. Lần chạy test đã bắt đầu trước chỉ thị mới dừng ở compile errors có sẵn trong `profile_runtime.rs` (`reserve_launch`, `launch_process`, thiếu `launching`); không sửa file ngoài scope. Cần lượt tích hợp cuối xử lý WIP compile state rồi chạy focused tests/gates.
- Chưa xác nhận cancellation ownership xuyên suốt upload caller; đó nằm ở đường worker orchestration ngoài scope. Retry budget cũng thuộc coordinator; projection chỉ trả phân loại lỗi hiện hành. Chưa commit/push/release.
- Residual fence: `clear_active_session(profile_id)` không nhận generation/attempt token, nên projection không thể phân biệt callback clear cũ với session mới. Cần integration owner đổi handoff/call-site contract ở lượt tích hợp; file ngoài scope không bị sửa tại đây.
- Rà tiếp stale hydrate: pending commit marker từng được cho qua nếu manifest trùng base, có thể khiến hydrate publication conflict thay dữ liệu cần recovery. Đã đổi thành lỗi `profile_local_sync_recovery_required`; manifest khớp target mới được đi tiếp, manifest khác bị stale-fence. Marker symlink/oversize/malformed hoặc checksum/generation sai trả integrity error rõ ràng.
- Đã đọc lại `CODEX_CHAT.md` sau cập nhật session 1; phần queue/lock timing của session 1 được giữ nguyên. Không chạy test/build/QA theo chỉ thị phiên.
- Bổ sung regression cho pending commit: hydrate manifest base giờ trả `profile_local_sync_recovery_required` và giữ nguyên bytes dirty, thay vì thử dựng base projection trong lúc commit local còn chờ reconcile. Hydrate đúng target vẫn được phép đi qua active marker base tương ứng; generation khác bị stale-fence.
- Bổ sung đo `profile_operations` queue/lock wait trong tổng launch attempt timer; log riêng thời gian chờ lock và tổng attempt, để lần sau xác định profile chậm ở hàng đợi hay preflight/runtime. Regression chạy lại sau thay đổi: transient projection test PASS 1/1. `cargo fmt --all -- --check` còn báo format trong các file WIP `profile_projection.rs` và `remote_api.rs` của session khác; file `desktop_state.rs` đã rustfmt riêng và diff check PASS.

## Điều phối 81: Session 4 — hoàn tất rà deadline refresh token
- Đọc lại `CODEX_CHAT.md` trước khi sửa; giữ nguyên thay đổi đồng thời của các session khác. Scope ghi vẫn chỉ `remote_api.rs`, `runtime_install.rs`, target selection trong `adapter.rs` nếu cần, và append chat log.
- Custom `device_session_refresh` nay giới hạn toàn bộ request, đọc body và backoff trong deadline 30 giây; timeout mỗi lần là 12 giây. Chỉ retry connect/timeout, HTTP 5xx, 408 hoặc 429; retry dùng cùng idempotency key, tối đa hai attempts, tôn trọng Retry-After cap 5 giây cộng jitter. 401/403/404 và response auth malformed/oversized không retry. Cancellation không tạo detached retry worker; request/retry còn thuộc async caller.
- Không thay crypto/KEK, fingerprint policy, API/server, UI hoặc dữ liệu production. Runtime contract và QA residual đã ghi ở Điều phối 78. Không chạy test/build/QA, không commit/push/release theo packet.
- Diff check cho hai file Rust trong scope sạch (chỉ cảnh báo chuẩn hóa CRLF). Chưa có xác nhận compile hay OS runtime QA; để lượt tích hợp cuối kiểm tra cùng WIP lifecycle khác.

## Điều phối 82: Session 2 — pending commit hydrate recovery
- Đọc lại CODEX_CHAT.md sau các cập nhật session 1/4; giữ nguyên mọi WIP ngoài `profile_projection.rs` và test cùng file.
- Hydrate cũ đã được fence trước network và trước cache return/publish bằng active-session + commit marker. Bổ sung phân biệt commit pending: manifest base trả `profile_local_sync_recovery_required`, đúng target được phép hydrate, generation khác trả `profile_hydration_stale`; marker symlink/oversize/malformed hoặc metadata sai không bị coi như marker vắng mặt.
- Thêm test giữ dirty bytes khi hydrate bị từ chối vì pending base. `git diff --check` scope sạch. Không chạy test/build/QA theo chỉ thị session; compile/test chưa được xác minh.
- Retry hữu hạn nằm ở coordinator ngoài write scope. `clear_active_session(profile_id)` vẫn không carry attempt/generation identity nên callback stale-clear cần integration handoff update ở lượt cuối.

## Điều phối 83: Session 2 — kiểm tra target hydrate đang commit
- Regression pending commit được mở rộng: dirty base bị chặn với lỗi recovery; manifest đúng target được cho qua khi active marker còn trỏ base đã ghi trong commit. Không chạy test/build/QA.
- `git diff --check -- apps/desktop/src-tauri/src/profile_projection.rs` sạch; phần logic mới chưa được compile/test xác nhận do yêu cầu chờ gate tích hợp.

## Điều phối 84: Session 4 — audit hoàn tất các deadline và pin runtime
- Đọc `CODEX_CHAT.md` nhiều lần trong lượt để giữ các cập nhật đồng thời. Audit source phát hiện timeout tổng ngoài ở generic send có thể cắt request ngay bằng timeout riêng, làm mất cơ hội retry timeout hữu hạn; đổi outer timeout sang remaining total deadline, giữ reqwest per-attempt timeout để phân loại đúng timeout từng lượt.
- Blob download trước dùng metadata timeout 30 giây; đổi sang transfer timeout 60 giây. SSE vẫn chỉ giới hạn bước mở kết nối 12 giây; body stream thành công không bị timeout cắt, còn body lỗi được giới hạn khi parse.
- `Retry-After` nay hiểu delta-seconds và HTTP-date, vẫn cap 5 giây + jitter. Device token refresh đọc body/error body trong tổng deadline, retry 5xx/408/429 hoặc lỗi connect/timeout đúng với stable idempotency key.
- Profile runtime pin thiếu target trước đây trả `runtime_not_configured` chung dù không fallback; `adapter.rs` nay trả `runtime_version_not_installed` kèm `runtimeVersion`. Cập nhật assertion hồi quy tương ứng, chưa chạy.
- Diff check các file trong scope sạch, chỉ cảnh báo CRLF. Không chạy test/build/QA, không commit/push/release. Win10/11, macOS Apple Silicon, fingerprint và KEK/legacy-envelope QA vẫn chờ tích hợp; runtime catalog signature, path confinement, executable hash/install receipt và host-target validation được giữ.
- Audit bổ sung: generic retry nay phân biệt per-attempt timeout với tổng deadline, và retry thêm 408/429 chỉ khi request safe/idempotent; Retry-After seconds/date vẫn bị cap. Blob GET dùng transfer budget 60 giây, lỗi TLS/non-transient không bị gắn retryable mù. Login/SSO POST không idempotency không phát tín hiệu retry; proxy parse safe POST retryable khi connect/timeout. Refresh token response malformed không phát tín hiệu retryable.
- Pinned runtime thiếu version được trả lỗi version-specific; vùng regression kiểm tra code + requested version. Không đổi chính sách cài, crypto/KEK hoặc fingerprint. `git diff --check` sẽ được kiểm tra cuối; test/build/QA vẫn bị cấm theo packet.
- Rà error classification: generic request chỉ trả retryable nếu operation safe/idempotent; generic 408/429 được retry tối đa một lần trong deadline. API login và BugLab SSO POST không idempotency không bật retry cho caller; proxy parse safe POST có retryable khi connect/timeout. Blob GET phân biệt lỗi connect/timeout/body với lỗi integrity/TLS.
- Event stream dùng một absolute 12s deadline cho cả mở kết nối và parse lỗi HTTP; body SSE thành công vẫn dài hạn. `git diff --check` cho remote_api/runtime_install/adapter sạch, chỉ cảnh báo line-ending CRLF. Không chạy test/build/QA theo yêu cầu; chưa xác nhận compile.
- Rà route `profile-runtime-status` vốn không chứa `/runtime/`, nên GET batch status trước bị xếp nhầm metadata 30s; thêm vào control/lifecycle class 12s. Giá trị Retry-After chuyển tiếp cho owner cũng cap 5 giây để không đẩy retry ngoài sang thời gian server gửi vô hạn.

## Điều phối 85: Session 3 — lease/runtime actor handoff
- Đọc lại CODEX_CHAT.md trước khi tiếp tục; chỉ sửa `profile_runtime.rs` và test cùng file. Không chạm `desktop_state.rs`, `profile_projection.rs`, `remote_api.rs` hoặc API server.
- Đưa lease acquire ra khỏi actor loop: actor chỉ reserve profile/session epoch, worker làm acquire/validate rồi gửi kết quả về actor; process chỉ launch sau khi lease hợp lệ. Thêm regression cho acquire chậm không chặn stop profile khác.
- Thêm monotonic `session_epoch` vào launch/stop receipt và RunningProfile. Renewal pending/completion được khóa theo epoch + fencing token + owner token; stale completion không clear trạng thái renewal của session mới dù lease được reuse.
- Thêm API stop/complete có expected epoch. Stale stop trả `profile_session_changed`; stale completion không xóa session mới. Regression mở lại bằng cùng LeaseGrant rồi gửi stop/complete/renew cũ.
- Remote lease release chạy ngoài actor sau khi state local đã chuyển; shutdown dừng process trước rồi release sau khi actor thoát. Giữ nguyên fencing, kiểm tra quyền và phân biệt lỗi lease ownership thật.
- `rustfmt --edition 2024 apps/desktop/src-tauri/src/profile_runtime.rs` đã chạy. Chưa xác nhận compile/test: cargo test mục tiêu dừng ở lỗi ngoài scope `profile_projection.rs:909`, `projection_state_problem` chưa được định nghĩa. Không sửa file session khác; cần chạy lại targeted test sau khi workspace tích hợp compile được.
- Tích hợp còn cần caller truyền `LaunchReceipt.session_epoch` vào `stop_for_session` / `complete_stop_for_session` hoặc detached tương ứng; legacy profile-id-only API vẫn giữ để tương thích. Chưa commit/push/release; đây là packet lifecycle runtime actor.

## Điều phối 86: Session 2 — phân loại lỗi projection để dừng retry mù
- Đọc Điều phối 83/84/85 trước khi sửa; giữ nguyên `profile_runtime.rs` và các WIP khác.
- Root cause retry: coordinator `profile_sync_error_should_retry` retry mọi lỗi HTTP 5xx; projection `problem()` mặc định gán 503 cả cho stale commit, stale hydrate, recovery-required, key/state conflict và corruption. Chúng tạo vòng thử lại vô ích sau khi profile đóng.
- Sửa projection phân loại state/conflict thành 409; marker, checksum, envelope, missing local generation và corruption thành 422; lỗi I/O transient còn 503. `profile_changed_during_snapshot` giữ 409 + `retryable=true` để coordinator retry hữu hạn khi file thực sự đổi trong lúc snapshot.
- Thêm test phân loại retry và assertion stale commit/hydrate/recovery không retryable. `git diff --check` scope sạch. Không chạy test/build/QA theo yêu cầu session; compile chưa xác nhận.
- Retry budget tổng vẫn thuộc coordinator ngoài scope; projection chỉ cung cấp classification đúng để retry scheduler không lặp conflict/corruption.

## Điều phối 86: Session 4 — cap Retry-After cho cả owner bên ngoài
- `read_problem` giờ cap `retryAfterSeconds` ở 5 giây dù giá trị đến từ header hoặc JSON problem body; tránh sync owner nhận delay server không giới hạn sau khi request client đã trả lỗi.
- Giữ `Retry-After` HTTP-date/delta parsing, backoff cap+jitter và mã lỗi auth 401/403/404 không retry. Không chạy test/build/QA.

## Điều phối 87: Session 2 — ngăn retry lỗi state/integrity
- Đọc CODEX_CHAT.md đến mục mới nhất 86 trước sửa; giữ nguyên entry session 4 và các WIP đồng thời.
- `profile_sync_error_should_retry` retry mọi 5xx. `profile_projection::problem()` mặc định trả 503, khiến stale commit/hydrate, conflict/recovery, key state, marker và checksum corruption bị retry như lỗi mạng. Nay state/conflict trả 409; integrity/corruption trả 422; transient projection I/O tiếp tục 503. Snapshot phát hiện file đổi giữ `retryable=true` để coordinator xử lý theo budget hữu hạn.
- Thêm regression classification và assertion non-retryable cho stale commit, stale hydrate, pending recovery; test chưa chạy theo chỉ thị. `git diff --check` cho projection sạch.
- Chưa xác minh compile/test. Bounded retry policy nằm ở coordinator ngoài scope; `clear_active_session(profile_id)` vẫn chưa có token để phân biệt callback clear cũ.
- Phân biệt corruption theo khả năng phục hồi: remote/decrypted integrity lỗi là 422 không retry; staged snapshot blob checksum lỗi trả 503 + retryable để dựng lại staging, còn coordinator vẫn áp retry budget tổng.

## Điều phối 88: Session 4 — bootstrap phục hồi khi runtime receipt hỏng
- Root cause: production `runtime_config::open()` gọi `load_installed_targets(...)?`; chỉ một receipt hỏng/chữ ký sai/hash mismatch có thể làm `build_state()` fail, khiến UI/runtime installer không lên để cài lại.
- Tách verify từng record thành `load_installed_target`; inventory chỉ nhận target qua signature, host, path confinement và executable hash/receipt. Record lỗi được log `code` rồi bỏ qua, inventory tốt khác vẫn dùng được; thư mục inventory lỗi/mục symlink hoặc không phải directory được bỏ qua có cảnh báo. Không chạy file chưa xác minh.
- Có regression trong `runtime_install.rs`: executable của một version bị sửa thì inventory giữ version verified và không chọn version hỏng. Không chạy regression/build/QA theo lệnh packet.
- Khi profile pin vào version bị loại, adapter trả `runtime_version_not_installed` kèm version; lệnh cài có thể cài lại đúng release qua staging/rollback. Giữ legacy layout, không xóa dữ liệu profile, không đổi crypto/KEK/fingerprint. Compile chưa xác minh.

## Điều phối 89: Session 2 — rà xác nhận retry policy projection
- Đọc Điều phối 87/88 và trạng thái worktree trước khi tiếp tục; không đụng file scope session khác.
- Xác nhận `profile_changed_during_snapshot` là retryable rõ ràng; `profile_sync_commit_stale`, `profile_hydration_stale`, `profile_local_sync_recovery_required`, marker/integrity errors không còn 5xx nên không kích hoạt rule retry 5xx của coordinator. Local staged blob checksum corruption vẫn được đánh dấu transient/rebuildable để bounded coordinator retry.
- Regression classification tự tạo staged bytes sai checksum và kiểm tra lỗi retryable; chưa chạy test/build/QA theo lệnh session. `git diff --check` cho projection sạch.

## Điều phối 90: Session 4 — từ chối idempotency key rỗng
- Generic API send trước coi mọi `Some(key)` là idempotent, kể cả chuỗi rỗng/chỉ whitespace. Nay reject `api_idempotency_key_invalid` trước network và chỉ bật retry khi có key không rỗng.
- `git diff --check` scope sạch. Không chạy tests/build/QA theo yêu cầu packet.

## Điều phối 91: Session 2 — giữ nguyên commit marker hỏng
- Đọc Điều phối mới nhất 90 trước khi sửa. `prepare_sync_commit` nay xác minh marker hiện hữu là regular file giới hạn kích thước, đúng profile, generation liền kề và checksum hợp lệ; marker hỏng không bị thay bởi retry mới.
- Thêm hồi quy xác nhận marker lỗi không retryable, được giữ nguyên byte-for-byte và dữ liệu dirty không đổi. `git diff --check` scope sạch; không chạy test/build/QA.

## Điều phối 92: Session 4 — giới hạn kích thước body lỗi API
- Đọc lại CODEX_CHAT.md trước khi sửa; giữ nguyên mọi WIP ngoài scope. `read_problem` trước deserialize toàn bộ response body không giới hạn, dù thời gian đọc bị deadline-bound.
- Nay đọc body lỗi theo stream với cap 64 KiB trước JSON parse. Body oversized hoặc body bị ngắt rơi về `api_request_failed`, vẫn giữ HTTP status, correlation, retry classification và Retry-After đã cap; request tiếp tục thuộc caller và deadline hiện tại.
- Không đổi API wire contract, không sửa server, crypto/KEK, fingerprint hay dữ liệu production. `git diff --check` cho file scope sạch (Git chỉ cảnh báo chuẩn hóa CRLF). Không chạy tests/build/QA theo packet; compile và OS runtime QA chưa xác nhận.

## Điều phối 93: Session 2 — fence commit theo active base
- Đọc Điều phối 91/92 trước sửa. `prepare_sync_commit` nay so active-session marker với base generation/root của upload trước khi ghi intent; nếu session đã sang generation khác thì trả stale 409, không ghi commit marker.
- Thêm test stale upload không tạo commit marker sau khi active session tiến. Marker absence vẫn hỗ trợ recovery/legacy caller; marker malformed bị fail-closed. `git diff --check` projection sạch; chưa chạy test/build/QA theo chỉ thị.
- Fence hiện dựa generation/root marker; caller không truyền session epoch nên cùng-generation reopen vẫn không thể phân biệt ở projection. Cần handoff epoch tại lượt tích hợp.

## Điều phối 94: Session 3 — acquire cancellation race và gate
- Đọc phần điều phối 90-93 trước khi tiếp tục. Trong `profile_runtime.rs`, khi stop đến lúc lease acquire đang chờ, đánh dấu launch epoch bị cancel nhưng giữ reservation đến khi worker trả kết quả; tránh vừa thả reservation vừa cho launch mới, vì release từ acquire cũ có thể đụng cùng owner/fence của launch mới.
- `LaunchAcquired` kiểm tra session epoch/cancel trước khi spawn process; acquire hoàn tất sau cancel thì release grant ở worker và trả `profile_launch_cancelled`. Thêm regression blocked acquire: stop profile trước khi acquire xong thì process không được launch. Giữ nguyên kiểm tra lease/fencing.
- Target riêng `E:\buglogin-target-session3` đã compile dependency; test chưa tới runner vì workspace compile fail ở WIP ngoài scope `remote_api.rs:3893-3929` (match `Option<ApiProblem>` bằng `Ok/Err`). `profile_projection.rs` blocker trước đó đã được sửa bởi owner session2. Chạy lại targeted runtime tests sau khi owner session4 sửa compile; không sửa file ngoài scope.

## Điều phối 95: Session 4 — retry status cuối 408 và sửa match response
- Đọc mục mới nhất 94 trước khi sửa. Rà phát hiện HTTP 408 được retry ở transport nhưng response cuối/ blob-upload retry owner nhận `retryable=false` nếu body không khai báo cờ; thêm helper classification thống nhất cho 408/429/5xx, vẫn chặn 401/403/404 và giữ explicit `retryable=false` từ server. Có regression trong `remote_api.rs`, chưa chạy theo packet.
- Cùng lúc phát hiện lỗi compile thật ở phần giới hạn problem body mới thêm: expression trả `Option<ApiProblem>` nhưng match arms còn `Ok/Err`. Đổi sang `Some/None`; lỗi này giải thích compile blocker session 3 báo tại remote_api.rs.
- `git diff --check` cần chạy cuối. Không chạy test/build/QA, không commit/push/release. Compile hiện chưa xác nhận; session 3 có thể chạy lại gate của họ sau khi file được owner thông báo.

## Điều phối 96: Session 4 — gỡ compile blocker được session 3 báo
- Sau khi đọc Điều phối 94/95, sửa match `Option<ApiProblem>` thành `Some/None`; đây là lỗi thực do bounded-body change, không phải lỗi session 3.
- `git diff --check` trên các file scope và CODEX_CHAT đã chạy sạch, chỉ có cảnh báo Git chuẩn hóa CRLF. Không chạy compile/test theo chỉ thị Session 4. Session 3 có thể tiếp tục lượt kiểm chứng runtime actor trên worktree tích hợp.

## Điều phối 96: Session 2 — serialize session marker transitions
- Đọc CODEX_CHAT.md đến Điều phối 95 trước khi sửa. `begin_active_session`, `clear_active_session_marker`, `clear_active_session` và `prepare_sync_commit` nay dùng cùng mutex của projection clone để tránh marker transition xen giữa bước check và ghi/xóa commit intent.
- Giữ active-generation check và stale commit rejection; không đổi lease/permission. `git diff --check` projection sạch. Không chạy test/build/QA theo chỉ thị.
- Mutex chỉ loại race thứ tự trong process; stale callback vẫn cần expected session epoch/generation tại API/caller để quyết định có được clear hay không.
- Mutex poison được phục hồi bằng guard còn giữ lock thay vì trả lỗi retryable/unavailable; panic trước đó không mở race giữa marker transition.

## Điều phối 97: Session 4 — xác thực idempotency key cho blob upload
- Đọc CODEX_CHAT đến Điều phối 96 mới nhất. `send_bytes` trước luôn phát retryable transport/error classification như thể upload có idempotency key, nhưng không từ chối key rỗng; giờ generic send và blob upload dùng chung predicate nonblank, upload dừng trước network với `api_idempotency_key_invalid` nếu key rỗng.
- Thêm focused regression cho key rỗng/whitespace và valid key; chưa chạy theo lệnh session. `git diff --check` cần kiểm tra sau entry này. Không chạy test/build/QA, không commit/push/release.

## Điều phối 98: Session 4 — audit transport/runtime tiếp tục
- Rà các callsite request trong `remote_api.rs`: SSE có deadline mở kết nối và giữ stream dài hạn có chủ ý; blob download/upload dùng transfer timeout; auth body và API problem body bị giới hạn; refresh-token replay dùng stable key, cùng tổng deadline. Không tìm thấy request lifecycle nào còn thiếu timeout trong các đường đã rà.
- `git diff --check` cho Session 4 scope và CODEX_CHAT đã pass (còn cảnh báo chuẩn hóa CRLF). Lỗi `Option<ApiProblem>` được sửa; Session 3 có thể tiếp tục kiểm chứng khi lượt của họ cho phép. Session 4 vẫn không chạy test/build/QA.

## Điều phối 99: Session 4 — residual runtime install/launch serialization
- Đọc CODEX_CHAT đến Điều phối 98. Audit call graph cho thấy `RUNTIME_INSTALL_LOCK` chỉ serialize installers; `AdapterState::launch_profile` không giữ cùng gate từ lúc chọn `LaunchSpecification` qua lúc `profile_runtime.launch` bàn giao process. `profile_launch` thường bảo đảm install thiếu runtime hoàn tất trước khi invoke, nhưng đường runtime install/update riêng có thể chạy song song với launch.
- Không sửa lifecycle handoff trong packet vì `adapter.rs` được giới hạn ở target selection và `profile_runtime.rs` thuộc Session 3; gate đúng cần cùng một permit được giữ xuyên tới spawn, không thể giải quyết bằng lock ngắn quanh lookup. Cần integration owner nối install writer gate với launch read permit, rồi regression cho update-vs-launch/cancel. Runtime install hiện vẫn versioned và verified, nhưng yêu cầu serialization này chưa được chứng minh/đóng trong Session 4 scope.

## Điều phối 100: Session 2 — serialize recovery check và marker clear
- Đọc Điều phối 96/98/99 trước sửa. `active_session_recovery_path` trước kiểm tra marker rồi gọi clear, cho phép `begin_active_session` chen vào giữa và marker mới bị xóa; nay recovery check + conditional clear giữ cùng commit mutex, gọi helper locked tránh deadlock.
- Serialize thêm `active_session_conflicts_with_manifest` và `verified_local_recovery_receipt` với session/commit transitions, tránh dựng recovery receipt từ marker đổi giữa các lần đọc.
- `git diff --check` projection sạch. Không chạy test/build/QA. Cùng-generation stale callback ở caller ngoài scope vẫn cần epoch token.

## Điều phối 101: Session 3 — cập nhật compile/test gate
- Đọc Điều phối 98-100. `cargo check --locked --manifest-path apps/desktop/src-tauri/Cargo.toml -p buglogin-desktop-app --lib` PASS trên Windows với target cô lập `E:\buglogin-target-session3`; có warning dead-code hiện hữu/WIP ở modules khác.
- `cargo test ... --lib profile_runtime::tests` chưa compile test target: snapshot hiện tại `remote_api.rs` unit tests gọi `problem_retryable` nhưng import `super::{...}` tại khoảng dòng 4427 chưa đưa tên này vào scope (`E0425`, 8 lỗi). Điều phối 98 ghi lỗi đã sửa nhưng source lúc Session 3 kiểm tra vẫn chưa có import. Không sửa file Session 4; rerun khi đồng nhật source.
- Compile target `G:\buglogin-target-session3-retry` trước đó thất bại do ổ G hết dung lượng; không xóa artifact. Test/check tiếp tục dùng E. Rustfmt check và diff check cho `profile_runtime.rs` sạch.

## Điều phối 101: Session 4 — deadline tổng cho runtime artifact download
- Đọc CODEX_CHAT đến Điều phối 100. `HttpsArtifactSource` trước đặt timeout 30 phút cho mỗi HTTP request nhưng resumable transfer có thể lặp request nhiều lần, nên cả thao tác tải không có một wall-clock deadline duy nhất.
- Thêm deadline tổng 45 phút cho artifact transfer; từng GET có timeout tối đa 30 phút và bị hạ theo remaining budget. Hết tổng thời gian trả `runtime_download_deadline_exceeded` (504), vẫn giữ resume/range checks, size/hash verification, staging và rollback hiện hành.
- `git diff --check` file runtime installer sạch. Không chạy tests/build/QA theo packet; compile và download trên Windows/macOS chưa xác minh.

## Điều phối 102: Session 4 — sửa imports của regression helpers
- Đọc cập nhật mới nhất của Session 3: test target không compile vì `remote_api.rs::tests` gọi `problem_retryable` và `valid_idempotency_key` nhưng chưa import hai helper từ parent module. Bổ sung imports trong test module thuộc đúng scope Session 4.
- `git diff --check` hai file scope sạch. Không chạy compile/test/build/QA; Session 3 có thể rerun test target trên `E:\buglogin-target-session3` sau khi nhận được cập nhật này.

## Điều phối 103: Session 2 — fence hydrate qua commit và publish
- Đọc Điều phối 100-102. Hydrate trước chỉ kiểm tra marker trước network, rồi publish sau network mà không đồng bộ với `begin_active_session`; session mới có thể bắt đầu giữa hai bước và generation hydrate cũ vẫn được publish.
- Nay tải/download giữ ngoài mutex; trước khi cache return hoặc publish, hydrate lấy commit mutex, kiểm tra lại active/commit marker và giữ lock qua local generation publication. Manifest rỗng cũng dùng cùng fence.
- `git diff --check` projection sạch. Không chạy test/build/QA theo chỉ thị; compile chưa xác minh. Verify sau rename có thể giữ mutex trong lúc đọc/hash local staged files, nhưng không giữ qua network.

## Điều phối 104: Session 2 — kiểm tra parent manifest và session marker
- Đọc Điều phối 103 trước sửa. `prepare_sync_commit` nay yêu cầu `target_manifest.parent_generation == Some(base.generation)`; test xác nhận target sai parent không tạo marker.
- `begin_active_session` trước dùng `Path::exists`/`read`, theo symlink marker và chấp nhận marker không giới hạn kích thước. Nay yêu cầu marker regular, non-symlink, <=64 KiB; malformed/invalid marker trả non-retryable 422, marker khác trả 409.
- `git diff --check` projection sạch. Không chạy test/build/QA theo chỉ thị.

## Điều phối 105: Session 3 — regression run và compile blocker kế tiếp
- Đọc Điều phối 102-104. Targeted `profile_runtime::tests` đã chạy: 36/38 pass. `stop_during_lease_acquire_cancels_launch_before_process_start` và stale session epoch regressions pass. Hai test khác ban đầu fail do assertion không tính profile thứ hai và timeout quá ngắn; đã sửa assertion/timeout, cần rerun.
- Rerun bị chặn trước test bởi WIP ngoài scope `runtime_install.rs:31-45`: `correlation_id` move vào `spawn_blocking`, sau đó closure `map_err` lại dùng; Rust báo E0382. Không sửa file Session 4. `cargo check --lib` đã PASS sau thay đổi launch reservation/cancellation.
- Không xóa target G đã hết dung lượng; build/test tiếp tục ở `E:\buglogin-target-session3`. Rerun ngay khi runtime_install test compile sửa xong.

## Điều phối 105: Session 4 — runtime update/launch cross-process gate
- Đọc CODEX_CHAT đến Điều phối 104. In-process mutex không ngăn hai BugLogin app process cùng máy mở/sửa runtime root. Dùng dependency `fs2` đã có sẵn: installer giữ exclusive lock file ở runtime root xuyên lượt install; launch lấy shared lock bằng `spawn_blocking` và fail-fast `runtime_install_in_progress` khi writer đang cài.
- Adapter giữ shared permit từ lúc chọn launch specification đến khi runtime actor trả kết quả spawn; file lock dùng chung giúp phối hợp nhiều app process. Thêm regression kiểm tra exclusive installer lock từ chối shared launch lock, rồi cho launch tiếp sau khi installer nhả lock.
- Cả installer và launch không chờ file lock vô hạn; integrity/staging/version checks giữ nguyên. `git diff --check` file scope sạch. Không chạy tests/build/QA theo packet; cần kiểm chứng contention và Windows/macOS semantics ở lượt tích hợp.

## Điều phối 106: Session 4 — sửa ownership correlation cho spawn_blocking
- Đọc Điều phối 105. Session 3 báo E0382 trong `acquire_runtime_launch_guard`: correlation string bị move vào closure nhưng vẫn được dùng để dựng lỗi JoinError. Tạo bản clone riêng cho worker, giữ correlation của caller cho nhánh join failure.
- `git diff --check` sẽ chạy sau lượt sửa này. Không chạy compile/test/build/QA trong Session 4; Session 3 có thể rerun targeted actor tests trên E sau cập nhật.

## Điều phối 107: Session 4 — diff check sau file lock và ownership fix
- `git diff --check` cho CODEX_CHAT và ba file scope `remote_api.rs`, `runtime_install.rs`, `adapter.rs` sạch; còn cảnh báo Git chuẩn hóa CRLF. `correlation_id` worker đã tách clone nên nhánh JoinError vẫn có correlation caller.
- Runtime operation lock dùng file lock chung theo runtime root; launch worker lấy shared lock không blocking và giữ file đến khi launch actor trả. Installer lấy exclusive lock không blocking trước inventory/install/reuse target; việc tạo runtime root và lock file xảy ra trước khi lock được giữ. Session 3 có thể rerun targeted tests trên E. Session 4 tiếp tục không chạy test/build/QA.

## Điều phối 108: Session 3 — targeted runtime actor hoàn tất
- Đọc Điều phối 106/107. `cargo check --locked --manifest-path apps/desktop/src-tauri/Cargo.toml -p buglogin-desktop-app --lib` PASS trên Windows; chỉ còn dead-code warnings ngoài phạm vi. `cargo test --locked --manifest-path apps/desktop/src-tauri/Cargo.toml -p buglogin-desktop-app --lib profile_runtime::tests` PASS: 38/38.
- Profile acquire/renew/release không còn chờ network trong actor; cleanup state local trước, release ở worker. Session epoch bảo vệ stale stop/complete/renew khi profile mở lại cùng lease. Stop trong lúc acquire hủy launch; reservation giữ đến sau lease cleanup để tránh release cũ va vào lần mở mới. Fencing và xử lý quyền/lease loss thật vẫn giữ nguyên.
- `rustfmt --check` và `git diff --check` cho `profile_runtime.rs` PASS. Chưa chạy cross-OS QA. Các caller trong `desktop_state.rs` hiện vẫn dùng API legacy theo profile ID; để stale callback bị fence đầu-cuối, lượt tích hợp phải truyền `LaunchReceipt.session_epoch` vào stop/complete session APIs. Không sửa `desktop_state.rs` theo ownership packet.

## Điều phối 109: Session 2 — giữ dirty local sau remote commit thành công
- Đọc Điều phối 104/107/108. Phát hiện lỗi recovery: nếu remote manifest đã bằng commit target, `active_session_recovery_path` trước xóa commit/active markers luôn. Nếu local generation base đã bị sửa sau lúc snapshot/upload, dirty bytes sẽ mất dấu dù vẫn còn trên đĩa.
- Nay nhánh remote-target kiểm tra local base với projection marker; chỉ xóa markers nếu base nguyên vẹn. Nếu base còn recoverable nhưng dirty thì chỉ xóa active marker, giữ durable commit marker, trả local path; recovery receipt trình bày path đó dưới identity manifest target để lượt stop tiếp theo snapshot dirty bytes trên đúng cloud base.
- Thêm regression cho commit thành công + thay đổi local sau snapshot: dirty bytes còn nguyên, receipt trỏ target generation, commit marker giữ lại, active marker được tách. Không chạy test/build/QA; `git diff --check` projection sạch.

## Điều phối 110: Session 4 — khóa runtime theo version để giảm chặn lifecycle
- Đọc CODEX_CHAT đến Điều phối 109, ownership manifest, plan/spec authoritative profile status gần nhất. File plan được nêu trong packet là `2026-10-28-authoritative-profile-status.md` không tồn tại; `2026-10-27-authoritative-profile-status.md` là plan hiện có.
- Root cause: file/process runtime lock trước dùng một khóa chung và installer giữ exclusive xuyên suốt tải artifact (deadline tối đa 45 phút). Vì vậy cập nhật một runtime có thể từ chối launch runtime version khác đang có sẵn, dù chúng không tranh cùng binary.
- Chuyển khóa process và file sang identity runtime + host OS + architecture + version với encoding OS/architecture cố định, không phụ thuộc Rust `Debug`. Installer và launch cùng tính key; launch chọn version từ target đã chọn rồi lấy shared lock trước khi dựng/validate launch specification, giữ đến khi runtime actor trả kết quả spawn. Cài mới một version không còn chặn version khác; xung đột cùng version vẫn fail-fast `runtime_install_in_progress`. Host OS/architecture vẫn lấy từ máy đang chạy app.
- Thêm regression helpers cho xung đột cùng key và độc lập giữa các version. Không chạy test/build/QA theo chỉ thị Session 4; chưa xác minh Rust compile hoặc semantics native Windows/macOS. Không đổi API transport/retry, fingerprint, crypto/KEK hay production data; không commit/push/release.

## Điều phối 111: Session 2 — rà recovery receipt và giới hạn fencing theo scope
- Đọc lại `CODEX_CHAT.md` từ Điều phối 109-110, `AGENTS.md`, ownership manifest, sync resilience spec/plan và diff projection trước khi chốt. Giữ nguyên WIP ngoài scope; không sửa file ngoài `apps/desktop/src-tauri/src/profile_projection.rs` và mục nối này.
- Rà `active_session_recovery_path`/`verified_local_recovery_receipt` với call-site trong `desktop_state.rs`: commit receipt chỉ được map sang target generation/checksum khi commit marker khớp target cloud; dirty base path giữ nguyên, marker commit còn để restart/Stop tiếp theo nhận ra dữ liệu cần sync. Không tạo manifest/receipt giả độc lập với durable commit marker.
- Integrity còn bắt buộc ở manifest validation, ciphertext/object checksum, authenticated decrypt, file checksum và projection marker. Recovery scan không coi thiếu/hỏng là profile rỗng; lỗi marker/file được giữ thành lỗi cần phục hồi.
- Hạn chế còn lại đã xác định: `clear_active_session*` chỉ nhận `profile_id`; callback cũ cùng ID và cùng generation/root chưa phân biệt được với session mới. Epoch/caller token cần tích hợp ở caller ngoài scope. Không nới/xóa kiểm tra để che race.
- `git diff --check` cho `profile_projection.rs` PASS; chỉ có cảnh báo Git chuẩn hóa LF/CRLF. Không chạy test/build/QA theo chỉ thị. Chờ lượt tích hợp cuối truyền session epoch cho commit/clear fences và xác minh Rust compile/test.
## Điều phối 112: Session 4 — tôn trọng retryable=false trong retry transport
- Đọc lại CODEX_CHAT đến Điều phối 111 trước khi tiếp tục. Audit `RemoteApiClient::send` phát hiện nhánh retry HTTP 502/503/504 bỏ qua `retryable: false` được API trả về, trong khi `read_problem` và lỗi cuối đã giữ cờ này. Một GET an toàn hoặc mutation có idempotency vẫn có thể bị replay ngoài chính sách của server.
- Nay nhánh retry đọc error body theo giới hạn 64 KiB và tổng deadline 30 giây trước khi quyết định; chỉ retry khi classification cuối cùng vẫn retryable. Giữ tối đa một retry, Retry-After cap 5 giây + jitter, các status nghiệp vụ 401/403/404 không retry, correlation và mã lỗi giữ nguyên.
- Thêm regression test cho 503 `retryable=false` xác nhận request không bị replay. Không chạy test/build/QA theo chỉ thị Session 4; chưa xác minh compile. Không thay đổi endpoint/server contract, crypto/KEK, fingerprint hay production data.

## Điều phối 112: Session 2 — cache hydration phải khớp tập file đã xác thực
- Đọc lại CODEX_CHAT từ Điều phối 111, kiểm tra code/diff projection và giữ nguyên các WIP ngoài scope.
- Root cause bổ sung: `verify_decrypted_generation` trước chỉ checksum các file có trong projection marker, không từ chối file thừa; cache có thể được coi là sạch dù thư mục chứa dữ liệu ngoài manifest. Nay kiểm tra tập đường dẫn thực tế khớp chính xác marker, từ chối marker trùng path và giữ file thừa nguyên trạng với `profile_projection_incomplete`.
- Thêm regression `verified_generation_rejects_untracked_local_files`, xác nhận file ngoài marker không bị xóa. Không đổi cache hỏng thành tải lại tự động: `local_generation_is_recoverable` không checksum từng file, nên không đủ chứng cứ để phân biệt corruption khỏi dirty user data; fail-safe conflict giữ dữ liệu.
- `git diff --check` projection PASS. Không chạy test/build/QA theo chỉ thị. Cần lượt tích hợp xác minh Rust compile/test; giới hạn caller epoch đã ghi tại Điều phối 111 vẫn còn.
## Điều phối 113: Session 4 — bounded retry cho tải runtime đã ký
- Đọc CODEX_CHAT đến Điều phối 112 mới nhất (bao gồm mục concurrent Session 2), rồi audit `HttpsArtifactSource::fetch_to`. Trước đó chỉ lỗi đọc stream có thể resume, còn lỗi connect/timeout ở `send`, HTTP 408/429/500/502/503/504 và stream ngắt trước byte đầu đều trả lỗi ngay.
- GET runtime artifact nay dùng chung ngân sách tối đa 4 retry cho connect/timeout, transient HTTP statuses và stream interruption; retry sau byte đầu tiếp tục bằng Range hiện có. Dùng exponential backoff + jitter, Retry-After numeric/date bị cap 5 giây, từng request bị giới hạn bởi remaining tổng deadline 45 phút. 401/403/404 và status không transient không retry; checksum/size/signature kiểm tra cuối vẫn bắt buộc.
- Thêm regression classification transient status; không chạy test/build/QA theo chỉ thị Session 4. Chưa xác minh compile, CDN resume thực tế hoặc Windows/macOS runtime install. Không thay file `crates/browser`, không đổi chữ ký/catalog contract hay production data.

## Điều phối 113: Session 2 — lỗi projection I/O phải được retry
- Đọc lại CODEX_CHAT đến Điều phối 112 và rà constructor `Problem` cùng lỗi projection. Root cause: `Problem::new` mặc định `retryable=false`; helper `unavailable()` trả HTTP 503 nhưng không đặt cờ retry, nên coordinator có thể dừng sau lỗi filesystem tạm thời dù trạng thái được báo là unavailable.
- Đặt `retryable(true)` cho lỗi projection I/O chung; giữ conflict (409) và integrity/corruption (422) non-retryable. Regression classification hiện xác nhận unavailable là 503 retryable, snapshot race retryable, conflict/corruption không retry. Không đụng quyền/lease hoặc file ngoài scope.
- `git diff --check` projection PASS. Không chạy test/build/QA theo chỉ thị; compile và regression cần lượt tích hợp cuối.
## Điều phối 114: Session 4 — giữ runtime permit qua caller cancellation
- Đọc CODEX_CHAT đến Điều phối 113 mới nhất, gồm concurrent Session 2. Audit handoff cho thấy `ProfileRuntimeHandle::launch` gửi lệnh vào actor rồi chờ oneshot; nếu adapter command bị cancel sau khi gửi, actor có thể tiếp tục trong khi guard thuộc future của caller bị drop.
- Adapter nay chuyển `LaunchRequest` và runtime guard vào task sở hữu lời gọi actor. Nếu caller bị hủy sau khi task được spawn, task tiếp tục giữ process/file shared permit đến khi actor trả kết quả hoặc task unwind; permit không bị thả sớm rồi cho installer thay binary cùng version trong lúc launch còn chạy. Không sửa `profile_runtime.rs`.
- `git diff --check` scope sạch. Không chạy test/build/QA; chưa xác minh compile/JoinHandle behavior trên Windows/macOS. Các thay đổi ngoài scope vẫn giữ nguyên.

## Điều phối 114: Session 2 — tách marker corruption khỏi lỗi I/O retryable
- Đọc lại CODEX_CHAT đến Điều phối 113 và rà các `serde_json::from_slice` trong recovery/cache path. Sau khi `unavailable()` thành retryable 503, vài nhánh parse projection/active/commit marker vẫn map JSON hỏng sang `unavailable`; corruption vì vậy bị retry lặp thay vì báo lỗi integrity.
- Các nhánh parse marker hỏng nay trả mã marker-invalid 422 non-retryable; lỗi đọc file vẫn giữ 503 retryable. Thêm regression `corrupt_projection_marker_is_not_classified_as_retryable_io`, kiểm tra marker không bị xóa/ghi lại.
- `git diff --check` projection PASS. Không chạy test/build/QA theo chỉ thị; chờ compile/regression ở lượt tích hợp cuối. Epoch fence caller vẫn ngoài scope.

## Điều phối 115: Session 2 — marker recovery không được biến mất thành cache miss
- Rà lại `verified_local_recovery_receipt` theo Điều phối 114. Phát hiện durable commit/session marker tồn tại nhưng identity sai hoặc generation/projection marker bị thiếu/hỏng có nhiều nhánh trả `Ok(None)`. Caller có thể hiểu là không có dữ liệu cần recovery rồi tiếp tục hydrate cloud, che corruption bằng cache miss.
- Khi recovery marker hiện diện, identity sai, local generation thiếu/invalid, projection marker thiếu/hỏng hoặc checksum/file set không recoverable nay trả lỗi 422 rõ ràng và giữ marker. Chỉ trả `None` khi thực sự không có commit/session marker. Thêm regression commit marker còn nguyên khi base generation bị thiếu.
- `git diff --check` projection PASS. Không chạy test/build/QA theo yêu cầu. Cần lượt tích hợp compile/regressions; epoch callback fence vẫn cần caller ngoài scope.
## Điều phối 116: Session 4 — giới hạn architecture detection trong write scope
- Đọc CODEX_CHAT đến Điều phối 115 mới nhất. Audit phát hiện `std::env::consts::ARCH` là architecture của binary được build, nên khác host khi app chạy qua translation/emulation. Đối chiếu tài liệu Microsoft: `GetNativeSystemInfo` cũng trả emulated view trong app chạy x64/x86 trên Windows ARM; API phù hợp là `IsWow64Process2`.
- Không thêm FFI: workspace đặt `unsafe_code = "forbid"`; packet cũng không cho sửa `Cargo.toml` để thêm wrapper dependency/API an toàn. Đã gỡ phần thử nghiệm FFI, giữ source chọn OS/architecture hiện có cho các bản app build native. Build native macOS arm64/Windows x64 chọn target tương ứng; app chạy giả lập chưa được phát hiện độc lập.
- Hỗ trợ architecture host dưới emulation cần owner mở write scope cho adapter/dependency manifest và kiểm chứng Windows ARM/Rosetta; hiện chưa có bằng chứng thiết bị bị ảnh hưởng. Không chạy test/build/QA theo chỉ thị Session 4.

## Điều phối 117: Session 4 — tổng kết source audit
- Đọc lại CODEX_CHAT đến Điều phối 116 và rà source hiện tại. `RemoteApiClient::send` giới hạn tổng retry, deadline cả lúc gửi lẫn đọc problem body, chỉ retry safe/idempotent; runtime artifact GET có retry/resume giới hạn và tổng deadline; launch/install cùng version được fence bằng process + file lock và guard sống qua actor task.
- Giữ các contract catalog/receipt/version và cấu hình runtime hiện hành; không đổi schema, API server, dữ liệu profile, fingerprint hay crypto/KEK. `std::env::consts::ARCH` vẫn chỉ đảm bảo host tương ứng khi desktop build native; under-emulation detection là residual ngoài source/API an toàn khả dụng trong write scope hiện tại.
- `git diff --check` đã chạy cho CODEX_CHAT và ba file được giao, không có lỗi whitespace (chỉ cảnh báo chuẩn hóa CRLF). Không chạy test/build/QA, không commit/push/release theo packet. Cần lượt tích hợp cho compile/tests và xác minh Windows/macOS, đặc biệt runtime file-lock semantics, caller cancellation và architecture under emulation.

## Điều phối 118: Session 4 — ma trận API/runtime trước lượt tích hợp
- Đọc CODEX_CHAT đến mục mới nhất (bao gồm Điều phối 117 của Session 2), rồi đối chiếu mọi đường request/runtime với packet.
- Lifecycle/control (lease, runtime projection/status, health/events): helper `send` chọn 12s theo path; tổng deadline retry 30s bao gồm send, problem body và JSON body. Tối đa một retry cho safe method/endpoint hoặc idempotency key; status 401/403/404 không retry, `retryable:false` của API được tôn trọng. Các HTTP auth/SSO trực tiếp cũng có timeout 12s; refresh có stable idempotency key và tổng deadline 30s.
- Catalog/metadata dùng 30s; blob download/upload dùng 60s. Download stream kiểm tra size/SHA-256 và báo retryability cho owner. Upload yêu cầu idempotency key nhưng không retry transport nội bộ; lỗi được trả về cho sync owner, tránh replay ở tầng transport ngoài lifecycle coordinator.
- Runtime host selection/install: catalog candidate được lọc runtime + host OS + process architecture + exact version; installer kiểm tra lại host tuple, chỉ nhận `VerifiedRuntimeRelease`, cài qua staging và xác minh signed package size/hash trước activate; persisted install record được kiểm tra signature, version/path confinement và executable SHA-256 khi load. Profile pin thiếu target trả `runtime_version_not_installed`, không fallback version.
- Cancellation/runtime lock: async API future bị drop không giữ retry loop; blocking installer tiếp tục chỉ trong worker sở hữu file/process lock. Launch task sở hữu guard tới actor result, nên caller cancel không thả khóa sớm. Process/file lock fail-fast, key cô lập theo runtime/OS/arch/version.
- Rà source `crates/browser/src/runtime/download` và `catalog` (read-only, ngoài write scope): ZIP/DMG archive signature xác thực package trước extract; executable receipt hash xác thực corruption sau cài. Receipt là local integrity receipt, không phải chữ ký riêng của executable; contract catalog hiện ký package-level digest, không có field signed per-binary digest. Không mở rộng schema theo packet. `git diff --check` cho file scope được giao chạy lại sau mục chat này; vẫn không test/build/QA.
- Chưa xác minh compile/native runtime-lock behavior và các lỗi filesystem/driver trên Windows/macOS. Architecture trong app emulation còn giới hạn như Điều phối 116/117. Không sửa file ngoài write scope, không commit/push/release.

## Điều phối 119: Session 4 — đối chiếu architecture với artifact release
- Đọc CODEX_CHAT đến mục mới nhất rồi đối chiếu `.github/workflows/signing.yml`, `rolling-release.yml` và `apps/desktop/src-tauri/runtime-catalog.json` (read-only, ngoài write scope). Stable signing matrix build Windows `x86_64-pc-windows-msvc` và macOS `aarch64-apple-darwin`; signed runtime catalog có Windows x64 và macOS arm64 tương ứng. Với stable artifacts hiện khai báo, `std::env::consts::ARCH` khớp runtime host vì mỗi binary được build native theo matrix.
- Rolling workflow còn build macOS x86_64, trong khi catalog tĩnh hiện chỉ khai báo macOS arm64; code lọc chính xác target nên bản rolling x86_64 sẽ nhận `runtime_release_not_found` thay vì âm thầm chạy runtime sai kiến trúc. Đây là thiếu catalog/release matrix ngoài write scope (catalog/workflow), chưa có bằng chứng rolling được phát hành tới người dùng trong task này.
- Kết luận cập nhật: không có mismatch OS/arch cho stable signing artifacts hiện tại; app tự chạy dưới Rosetta/Windows ARM emulation vẫn chỉ nhận biết process architecture và chưa được hỗ trợ native-host selection. Giữ lỗi rõ khi không có target đúng, không fallback. Chưa chạy test/build/QA; không sửa workflow/catalog hay file ngoài scope.

## Điều phối 117: Session 2 — recovery marker read không nuốt I/O hoặc identity lỗi
- Đọc lại CODEX_CHAT đến Điều phối 115 và kiểm tra `active_session_recovery_path`, `verified_local_recovery_receipt`, `active_session_conflicts_with_manifest`. Tìm thấy `fs::read(...).ok()`/`if let Ok` có thể coi commit marker lỗi permission/I/O như marker vắng và fallback sang active marker; vài read cũng bỏ giới hạn kích thước hoặc kiểm tra symlink.
- Thêm `read_projection_marker`: phân biệt missing với I/O retryable; yêu cầu regular non-symlink file, tối đa 64 KiB; file invalid/oversize trả 422. Nối vào ba recovery/conflict paths. Thêm regression oversized marker được giữ nguyên và không retry.
- `git diff --check` projection PASS. Không chạy test/build/QA. Compile/regression chưa xác minh; session epoch vẫn cần caller ngoài scope.

## Điều phối 120: Session 4 — refresh phải tôn trọng retryable=false
- Đọc CODEX_CHAT đến mục mới nhất trước khi sửa. Audit phát hiện `device_session_refresh` có stable idempotency key nhưng gọi `read_problem(..., false)`, sau đó ép mọi HTTP 5xx/408/429 thành retryable và replay kể cả khi API trả rõ `retryable:false`. Điều này bất nhất với generic transport và server contract.
- Refresh nay đọc problem dưới retry-safe classification dựa trên stable idempotency key, retry transient statuses tối đa một lần chỉ khi `problem.retryable` còn true; cờ explicit false, code và correlation từ server được giữ nguyên. Mở rộng regression: retryable 503 vẫn retry cùng key; explicit nonretryable 503 chỉ nhận một request và trả nguyên business code/correlation.
- Thay đổi chỉ trong `remote_api.rs` và mục chat này. Không chạy test/build/QA theo packet; regression mới chưa thực thi. `git diff --check` sau cập nhật PASS, chỉ còn cảnh báo chuẩn hóa CRLF của Git.

## Điều phối 121: Session 4 — xác minh retry refresh giữ nguyên idempotency identity
- Đọc CODEX_CHAT đến Điều phối 120 trước khi tiếp tục. Siết regression retryable 503: ngoài việc nhận đủ hai request, server harness trích `Idempotency-Key` từ từng request và yêu cầu key bằng nhau. Điều này xác minh retry refresh dùng cùng identity ổn định, không chỉ tạo hai request có key bất kỳ.
- Không chạy test/build/QA theo packet; regression đã được thêm nhưng chưa thực thi. `git diff --check` sau cập nhật PASS (chỉ cảnh báo CRLF).

## Điều phối 122: Session 4 — jitter cho retry transport API
- Đọc CODEX_CHAT đến Điều phối 121 trước khi sửa. Generic API send và refresh token trước đều đợi cố định 250 ms sau connect/timeout/body interruption; response retry đã có correlation jitter. Nhiều client vì vậy có thể đồng bộ lại cùng lúc sau transport outage.
- Thêm `api_transport_retry_delay`: bounded 250–750 ms delay, jitter từ correlation; áp dụng cho generic safe/idempotent send và các nhánh transport/timeout/body interruption của refresh. Số lần retry/tổng deadline/key/error classification không đổi; Retry-After response vẫn ưu tiên theo helper riêng và cap hiện hành.
- Không chạy test/build/QA theo packet. Chạy `git diff --check` sau mục chat này.

## Điều phối 118: Session 2 — upload giữ immutable ciphertext cache path
- Đọc lại CODEX_CHAT đến Điều phối 117. Rà cancellation/retry của upload: `SnapshotBlob` trước trỏ vào cache alias theo plaintext; nếu alias được thay trong lúc worker cũ còn upload thì checksum fail và lượt retry phải dựng lại snapshot, làm chậm recovery.
- Snapshot vẫn đọc cache alias tương thích hiện hành, nhưng nay đồng thời ghi ciphertext vào path content-addressed theo checksum và `SnapshotBlob` upload trỏ path immutable đó. Thay alias ở snapshot sau không sửa bytes đang được upload; bytes cache cũ được giữ lại cho worker/retry đã nhận path. Thêm regression xác nhận path đầu vẫn giữ bytes sau khi cùng alias nhận payload mới.
- Không đổi schema/receipt, key/nonce hay lease/quyền. `git diff --check` projection PASS. Không chạy test/build/QA; compile/regression còn chờ tích hợp.

## Điều phối 123: Session 4 — chặn symlink ở runtime operation lock file
- Đọc CODEX_CHAT đến mục mới nhất trước khi sửa. Audit `confined_path` cho lock leaf phát hiện dangling symlink làm `Path::exists()` false; `OpenOptions::create(true)` cũ có thể theo symlink và tạo/mở file ngoài runtime root.
- Thay bằng mở lock file có kiểm tra `symlink_metadata`: existing target phải regular file; missing path được tạo atomic qua `create_new`; race AlreadyExists được recheck giới hạn ba lần; symlink/directory/non-regular path fail rõ. Thêm Unix regression cho dangling symlink và xác nhận external target không được tạo.
- Chỉ sửa `runtime_install.rs` và mục này. Không chạy test/build/QA theo packet; regression chưa thực thi. `git diff --check` sau mục chat này.

## Điều phối 119: Session 2 — publish ciphertext cache không ghi đè upload đang chạy
- Đọc lại CODEX_CHAT đến Điều phối 118 và xác minh caller cancellation path: staged-file reader chạy blocking task sở hữu; cancellation có thể cho lượt mới chạy trong lúc reader cũ còn sống. Vì cache alias theo plaintext và AEAD ciphertext có nonce riêng, lượt cũ/lượt mới có thể tham chiếu cùng alias nhưng checksum khác, gây retry vô ích.
- `SnapshotBlob` nay trỏ path content-addressed theo ciphertext checksum. Publish immutable dùng `create_new`; path có sẵn chỉ tái dùng khi checksum/size khớp, collision/corrupt path chọn tên riêng bounded. Cache alias legacy vẫn là lookup; hit không fsync/ghi lại alias, miss mới cập nhật alias. Hỗ trợ cả object đơn và file chunk.
- Regression xác nhận hai ciphertext cùng cache alias nhận path riêng, bytes path cũ bất biến, alias cập nhật payload mới. Không đổi format manifest/object/key/encryption/lease/permission.
- `git diff --check` projection PASS. Không chạy test/build/QA. Compile/regression cần lượt tích hợp.

## Điều phối 124: Session 4 — confinement phải bắt đầu từ runtime root
- Đọc CODEX_CHAT đến mục mới nhất trước khi sửa. `confined_path(runtime_dir, relative_executable)` canonicalize `runtime_dir` thành root mới; nếu `root/wayfern/<os>/<arch>` hoặc `versions` là symlink, nó có thể chấp nhận cây ngoài runtime root. Inventory cũng `read_dir(versions)` trước khi xác thực parent chain.
- Thêm `confined_runtime_path(root, path)` để kiểm tra mọi component tương đối từ canonical runtime root. Inventory canonicalize root, xác nhận host/versions tree trước traversal; record và executable receipt được resolve lại từ root; installer canonicalize root và kiểm tra release directory trước staging; existing-install reuse cũng kiểm tra directory/record/executable từ root.
- Thêm Unix regression để inventory từ chối host OS directory symlink. Không đổi runtime catalog/schema/receipt format và giữ record legacy hợp lệ. Không chạy test/build/QA theo packet; `git diff --check` sau mục này.

## Điều phối 125: Session 4 — lỗi inventory không được biến thành runtime cache miss
- Đọc CODEX_CHAT đến Điều phối 124 trước khi sửa. Thêm kiểm tra runtime root phải là directory; thiếu root thật sự vẫn là inventory rỗng. Host directory không tồn tại cũng trả rỗng, nhưng file thay directory, permission/read_dir lỗi hoặc inventory entry I/O lỗi nay trả lỗi path cụ thể thay vì âm thầm bỏ target.
- Legacy record và version record được dò bằng `symlink_metadata`, phân biệt NotFound với I/O; file record symlink vẫn chuyển tới loader để bị từ chối bằng confinement. Thêm regression root là file phải trả `runtime_install_path_invalid`.
- `git diff --check` sau cập nhật PASS (chỉ CRLF normalization warnings); không test/build/QA theo packet. Không đổi schema, chữ ký, receipt format hay dữ liệu runtime.

## Điều phối 126: Session 4 — receipt/runtime I/O lỗi phải lộ rõ
- Đọc CODEX_CHAT đến Điều phối 125 và rà lại các nhánh nhận diện runtime đã cài. `existing_install` trước đó biến lỗi `symlink_metadata`/đọc receipt không phải NotFound thành cache miss, có thể khởi chạy cài lại mà không báo nguyên nhân permission/I/O.
- Nay chỉ receipt thật sự vắng, sai cấu trúc/metadata hoặc executable thiếu/hỏng được coi là cần phục hồi bằng đúng release đã xác minh; receipt không phải regular file và lỗi inspect/read ngoài NotFound trả `runtime_install_record_invalid` hoặc `runtime_install_record_unavailable`. Lỗi inspect executable ngoài NotFound trả `runtime_executable_invalid`. Không dùng binary hỏng; staging/rollback và host/version pin hiện có được giữ nguyên.
- Audit request kết thúc: lease acquire/renew/release và manifest commit đều qua `send` có deadline toàn phần 30s; lease dùng idempotency key caller, commit dùng key ổn định từ manifest; 401/403/404 không retry, `retryable:false` được giữ, mutation upload blob không tự replay. GET blob/catalog, auth/control và runtime artifact đều có timeout hữu hạn; runtime download có tổng deadline/retry/resume hữu hạn. Không phát hiện thêm lỗi transport cụ thể trong scope.
- `git diff --check` scope Session 4 PASS (chỉ cảnh báo Git chuẩn hóa CRLF). Không chạy test/build/QA đúng packet; compile, test regression, race/file-lock behavior và Windows/macOS native verification vẫn chờ lượt tích hợp. Không sửa fingerprint/KEK/E2EE, server/API, catalog/workflow hay production data; không commit/push/release.

## Điều phối 120: Session 2 — commit cần active marker của đúng base
- Đọc CODEX_CHAT đến Điều phối 119. Audit `prepare_sync_commit` phát hiện khi active-session marker đã bị clear/mất, code vẫn có thể ghi commit marker chỉ dựa vào base/target manifest. Worker upload cũ có thể promote marker sau khi session kết thúc.
- Nay commit chỉ được tạo nếu active marker hiện hữu, hợp lệ và trùng base generation/root. Thiếu marker trả stale 409, không retry mù và không tạo marker. Retry chính xác có commit marker bền vững đã tồn tại vẫn idempotent. Cập nhật setup regression hiện hành và thêm `stale_upload_cannot_prepare_commit_after_session_marker_is_cleared`.
- `git diff --check` projection PASS; static scan mọi `prepare_sync_commit` test thấy setup active marker phù hợp. Không chạy test/build/QA theo yêu cầu. Epoch vẫn chưa tách được hai session cùng profile/base; cần API caller token ở lượt tích hợp ngoài scope.

## Điều phối 127: Session 4 — bàn giao recovery runtime hỏng sau bootstrap
- Đọc CODEX_CHAT đến mục mới nhất và rà call path `profile_launch`/`ensure_profile_runtime_installed` (read-only ngoài scope). Preflight quyết định cần cài dựa trên `runtime_installed_versions_get`; danh sách này là inventory đã nạp lúc bootstrap. Nếu executable bị sửa/hỏng sau bootstrap, launch vẫn kiểm tra SHA-256 trong `WayfernAdapter::validate_binary` và từ chối chạy, nhưng preflight chưa tự làm version biến mất khỏi inventory để cài lại ngay trong cùng lượt.
- `runtime_install` cho phép cài lại cùng exact signed release; `existing_install` bỏ qua receipt/checksum hỏng để đi qua transactional staging/rollback. Vì Session 4 chỉ được ghi `remote_api.rs`, `runtime_install.rs`, target selection trong `adapter.rs`, không sửa `lib.rs`/UI để tự gọi repair sau lỗi checksum. Tích hợp cuối nên nối lỗi `runtime_checksum_mismatch`/`runtime_executable_invalid` với cài lại đúng pinned signed release rồi cho người dùng thử launch lại; không fallback sang version khác. Hiện hành vi từ chối binary hỏng đã có, auto-repair cùng lượt còn thiếu.
- Đã rà `adapter.rs`: profile pin chọn đúng key version và trả `runtime_version_not_installed` nếu không có; unpinned chọn version mới nhất trong inventory đã xác minh. Không dùng OS fingerprint để chọn host binary. Không có thay đổi code ở mục này; `CODEX_CHAT.md` ghi lại residual và owner cần tích hợp.

## Điều phối 121: Session 2 — integrity marker thống nhất qua cache/recovery
- Đọc CODEX_CHAT đến Điều phối 120 và rà marker reads còn trực tiếp trong verify/cache path. `verify_decrypted_generation` có thể theo symlink hoặc parse marker không giới hạn kích thước; marker tương tự được xử lý khác nhau giữa cache verification và recovery receipt.
- Nối `read_projection_marker` vào `verify_decrypted_generation`, `local_generation_matches_projection_marker`, `local_generation_is_recoverable` và nhánh projection marker trong `verified_local_recovery_receipt`. Thiếu marker vẫn phân biệt theo contract từng call-site; symlink/non-regular/oversize/malformed trả integrity error, I/O trả transient 503.
- `git diff --check` projection PASS. Không chạy test/build/QA; regression 64 KiB trước đó dùng chung helper, compile vẫn chờ lượt tích hợp.

## Điều phối 122: Session 2 — recovery không trả path khi projection marker hỏng
- Đọc CODEX_CHAT đến Điều phối 121. Trong hai nhánh active/pending-session cùng remote base, `verify_decrypted_generation(...).is_ok()` từng làm mọi lỗi (kể cả marker thiếu/hỏng) rơi xuống `Ok(Some(path))`, khiến caller mở local tree mà integrity chưa được xác nhận.
- Thay bằng kiểm tra projection marker/path recovery: generation sạch được xác nhận và marker stale được clear; dirty file set được trả làm recovery path khi projection marker còn hợp lệ; marker thiếu/hỏng hoặc file thiếu trả lỗi non-retryable, dữ liệu/commit marker giữ nguyên. `local_generation_is_recoverable` nay cũng từ chối file được marker tham chiếu nhưng bị thiếu.
- Thêm `pending_commit_recovery_surfaces_corrupt_projection_marker`, xác nhận lỗi 422 và dirty bytes/commit marker vẫn còn. `git diff --check` projection PASS. Không chạy test/build/QA; compile/test chờ lượt tích hợp.

## Điều phối 123: Session 2 — recovery marker từ chối duplicate path
- Đọc CODEX_CHAT đến Điều phối 122. Audit `local_generation_matches_projection_marker` và `local_generation_is_recoverable`: cả hai so marker với file set nhưng không phát hiện hai `local_objects` trùng path. Marker hỏng có thể được dùng để clear active session như generation sạch hoặc nhận làm recovery.
- Hai validators nay báo `profile_projection_marker_invalid` 422 khi marker chứa duplicate path; file thiếu/extra tiếp tục không được coi là clean/recoverable. Thêm `recovery_rejects_projection_marker_with_duplicate_paths`, kiểm tra local bytes còn nguyên.
- `git diff --check` projection PASS. Không chạy test/build/QA theo yêu cầu; compile/regression vẫn chờ lượt tích hợp.

## Điều phối 124: Session 2 — immutable cache publish atomically
- Đọc CODEX_CHAT đến Điều phối 123. Rà crash consistency của content-addressed ciphertext cache: bản đầu ghi trực tiếp vào `create_new` path cuối; app crash giữa `write_all`/`sync_all` có thể để cache object dở dang.
- Nay cache alias giữ tương thích như cũ; immutable path được tái dùng chỉ khi size/checksum đúng, còn thiếu/hỏng thì publish qua `write_verified` (temporary + atomic persist) và xác minh path lại. Payload checksum trong tên vẫn tách ciphertext khác nhau; worker không đọc file staging dở dang.
- `git diff --check` projection PASS. Không chạy test/build/QA; không đổi protocol/manifest/lease/quyền.

## Điều phối 125: Session 2 — regression cache crash artifact
- Đọc CODEX_CHAT đến Điều phối 124. Bổ sung case vào `snapshot_cache_paths_are_immutable_across_payload_replacement`: giả lập content-addressed path dở dang, gọi cache repair và xác nhận đúng immutable path nhận lại payload có checksum dự kiến.
- `git diff --check` projection PASS. Không chạy test/build/QA theo yêu cầu.

## Điều phối 126: Session 2 — giữ classification lỗi ghi cache
- Đọc CODEX_CHAT đến Điều phối 125. Root cause retry lặp: các snapshot cache writers bọc mọi `write_verified`/`confined_path` error thành `profile_projection_unavailable` 503 retryable. Storage-full (507) và permission-denied (500) vì vậy bị schedule retry như lỗi tạm thời.
- Cache writers nay giữ nguyên `Problem` gốc; chỉ `blob_write_failed` và `storage_root_unavailable` được đánh dấu transient retryable. `device_storage_full`, permission, path/integrity errors giữ mã/status và không retry. Regression classification bổ sung các lớp lỗi này.
- `git diff --check` projection PASS. Không chạy test/build/QA theo yêu cầu.

## Session 2 — hoàn tất rà static profile projection
- Scope ghi: `apps/desktop/src-tauri/src/profile_projection.rs`, test cùng file nếu cần; `CODEX_CHAT.md` chỉ ghi session/tiến độ. Đã đọc lại ownership, spec thiết kế unified profile sync, plan unified sync, plan resilience và CODEX_CHAT trước khi sửa.
- Rà lỗi sửa dở ở snapshot cache: `io_problem(error)` không tồn tại. Thay bằng `snapshot_cache_io_problem`, phân biệt storage full (507), permission (500), lỗi IO khác (`blob_write_failed`); chỉ lỗi IO ghi transient tiếp tục retry qua `snapshot_write_problem`. Các `write_projection_verified` đều đi qua classification chung; integrity/path errors giữ nguyên.
- Nguyên nhân/hành vi đã xử lý trong file scope: hydrate/recovery không nuốt marker/generation lỗi hoặc trả profile rỗng; cache hydrate xác minh file set; snapshot upload dùng ciphertext path bất biến và cache publish atomic; commit đòi active-session base chính xác, conflict/stale không đánh dấu sạch; state retry/recovery/marker giữ tương thích hiện hành. Regressions nguồn cho corruption, missing/duplicate data, stale state, cache race và commit recovery có trong cùng file.
- Giữ schema/receipt/recovery receipt hiện hành, legacy cache lookup, manifest/key/encryption, lease/permission, không tạo manifest/key giả, không bỏ integrity checks, không xóa legacy data. WIP ngoài phạm vi giữ nguyên.
- `git diff --check` cho hai file trong scope thành công (chỉ cảnh báo line ending CRLF của Git). Không chạy test/build/QA theo yêu cầu; compile và regressions chưa xác minh. Residual integration: fence giữa hai session cùng profile/base cần epoch/identity do caller ngoài scope cung cấp; end-to-end retry sau restart và real-transfer/process-death vẫn chờ lượt tích hợp cuối.

## Session 2 tiếp tục — giữ dirty generation khi kiểm tra recovery
- Đọc lại CODEX_CHAT và đường `local_generation_matches_projection_marker` / `local_generation_is_recoverable`. Rà soát phát hiện không được so bytes dirty với checksum file của base để quyết định khả năng recovery: browser có thể sửa/xóa/thêm file sau khi marker base được ghi. `local_generation_matches_projection_marker` vẫn xác minh tập file và checksums để nhận diện generation sạch; recovery chỉ dựa trên marker base hợp lệ và duyệt filesystem an toàn, giữ thay đổi browser để snapshot tiếp theo.
- Thêm regression `changed_generation_file_is_not_mistaken_for_clean_recovery_state`: sửa bytes tại chỗ, xác nhận không clean nhưng local generation vẫn recoverable và dirty bytes còn nguyên.
- Không thay quyền/lease/schema/receipt; không test/build/QA. Chưa xác minh compile. `git diff --check` sau mục này.

## Session 2 tiếp tục — giới hạn đọc SnapshotBlob
- Đọc lại CODEX_CHAT trước khi sửa. `SnapshotBlob::read_bytes` gọi `fs::read` trực tiếp trước khi so kích thước; file cache hỏng ngoài băng có thể cấp phát vượt kích thước manifest/object hợp lệ.
- Thêm kiểm tra regular non-symlink, metadata size khớp receipt và không vượt `MAX_ENCRYPTED_OBJECT_BYTES` trước khi cấp phát; đọc thêm tối đa size+1, giữ checksum validation. Thêm regression nguồn oversized cache bị từ chối trước khi đọc.
- Không đổi file khác, schema/receipt/keys/lease/quyền. Không chạy test/build/QA; compile chưa xác minh. `git diff --check` hai file scope PASS (cảnh báo line-ending CRLF từ Git).

## Session 2 tiếp tục — giữ lỗi hydrate cache và fence filesystem
- Đọc lại CODEX_CHAT, hydrate cache và các existence checks. Cached generation trước đó dùng `verify_decrypted_generation(...).is_ok()` nên nuốt marker/I/O/integrity error thành cache miss rồi trả conflict chung; nay chỉ file-set/data mismatch đi theo recovery-required, lỗi marker/I/O cụ thể được trả nguyên trạng.
- Thay `Path::exists()` trên generation paths dùng bởi hydrate/recovery/publish bằng `symlink_metadata` kiểm tra regular directory. Symlink, file hoặc path không hợp lệ không còn được đọc như cache ngoài root; verifier xác nhận generation directory trước khi mở marker. Snapshot đọc kết quả verify từ blocking worker: lỗi marker/I/O được giữ nguyên, chỉ file mismatch vào recovery-required. Recovery giữ dirty bytes. Thêm regression source cho generation symlink.
- `git diff --check` sau cập nhật; không chạy test/build/QA. Compile/regression chưa xác minh. Sync epoch phân biệt hai worker cùng profile/base vẫn cần caller contract ngoài scope; không đổi quyền/lease.
- Rà lại toàn bộ `Path::exists()` còn lại: ba call-site trong recovery đều gọi validators dùng `symlink_metadata`; phần còn lại chỉ test cleanup. `git diff --check` scope PASS.

## Session 2 tiếp tục — fence durable cho hydrate generation cũ
- Đọc lại CODEX_CHAT và hydrate multi-call paths. Active-session/commit marker không tồn tại trong cold open; vì thế hydrate generation cũ có thể chạy chậm hơn hydrate mới, rồi publish sau đó nếu trước publish chưa có session marker.
- Thêm marker fence nội bộ profile-scope lưu generation/root checksum lớn nhất đang hydrate. Cập nhật nguyên tử dưới commit lock; retry cùng generation/root idempotent; generation thấp hơn hoặc cùng generation khác root trả `profile_hydration_stale` 409. Hydration kiểm tra fence trước khi bắt đầu và trước publish/cache reuse; marker hỏng trả integrity error, không bị xóa.
- Active-session start cũng kiểm tra/advance fence; finalize commit sau API success khi clear marker advance fence tới target receipt. Recovery xác nhận target từ remote cũng advance fence trước khi dọn marker. Callback hydrate cũ không hồi quy về base sau reopen. Fence là cache coordination metadata, không thay schema manifest/receipt/key/lease/quyền. Regression source xác minh retry idempotent, generation fork/old session bị chặn, fence durable qua reopen và commit clear giữ target. `git diff --check` sau cập nhật; không chạy test/build/QA, compile chưa xác minh.

## Session 2 tiếp tục — phân loại lỗi đọc snapshot source/cache
- Đọc lại CODEX_CHAT và toàn bộ source snapshot/read path. `read_dir`, metadata, open/read cho profile source từng gộp lỗi mọi loại thành 503 retryable; permission bị retry vô ích. Kích thước file lớn được scan trước rồi mở sau, nên file biến mất/truncate phải retry như thay đổi concurrent; giờ `NotFound`/`UnexpectedEof` map vào `profile_changed_during_snapshot`, permission/storage-full/invalid-data dừng retry với lỗi cụ thể.
- `SnapshotBlob::read_bytes` cũng phân biệt cache mất (rebuild retryable) với permission denied (non-retryable), đồng thời giữ giới hạn kích thước/checksum. Thêm regression nguồn cho missing/truncated, transient IO, permission và missing/permission cache.
- Chỉ `profile_projection.rs` và mục chat. `git diff --check` sau sửa; không test/build/QA theo chỉ thị, compile/regression chưa xác minh. Không đổi lease/quyền/protocol.

## Handoff qua CODEX_CHAT — Session 2
- Ghi chú trao đổi: khi retry profile sync sau khi đóng profile, hãy phân biệt lỗi source trước khi retry. Permission trên file profile/cache là lỗi hành động cần xử lý, không phải outage tạm thời; `NotFound`/`UnexpectedEof` trong lúc snapshot là dấu hiệu file đổi giữa scan/read, cho phép dựng snapshot mới.
- Patch hiện tại áp dụng phân loại này trong `collect_files`, `collect_file_paths`, source readers và `SnapshotBlob::read_bytes`; regressions nguồn bao gồm missing/truncated/transient/permission/cache-missing. `git diff --check` đang được rà sau nội dung trao đổi này.
- Handoff integration: old upload worker cùng profile/base chưa thể phân biệt reopen epoch từ API `profile_projection.rs` hiện tại; caller cần truyền epoch/token ở lượt tích hợp, đồng thời giữ nguyên contract lease/API.

## Session 2 tiếp tục — static audit snapshot source
- Đọc lại mục handoff và source hiện hành. Xác nhận encrypted snapshot inventory giữ `(path,size)` rồi đọc từng file với size fence; file disappeared/truncated thành retryable `profile_changed_during_snapshot`; permission thành lỗi non-retryable. Không thêm loop retry trong projection, để coordinator hiện hành giữ quyền điều phối và giới hạn số lượt.
- `git diff --check` hai file scope PASS (chỉ CRLF warning). Không chạy test/build/QA theo yêu cầu. Hạn chế còn mở: không thể token-fence old upload cùng profile/base trong file này vì API projection không nhận session epoch; caller/integration cần bổ sung.
- Follow-up: source có thể thay đổi cùng lúc mà giữ nguyên size giữa inventory và read. Readers so `len`/`modified` trên path và open handle trước/sau; small-file read bị giới hạn `expected_size + 1`; large-file stream cũng xác minh metadata path/handle. Mismatch trả retryable `profile_changed_during_snapshot`, không bỏ mất dirty input. Regression source bao phủ drift size.

## Session 2 — đồng bộ profile và tính toàn vẹn dữ liệu
- Scope: `apps/desktop/src-tauri/src/profile_projection.rs`, regression cùng file, mục append này trong `CODEX_CHAT.md`. Đọc lại ownership, plan/spec sync resilience và nhật ký trước chỉnh sửa; giữ nguyên WIP ngoài scope.
- Root cause retry: `ResourceKeyEnvelope::wrap` mã hóa ngẫu nhiên nên tạo ciphertext/checksum/root mới cho cùng resource key mỗi snapshot retry. Projection nay cache envelope theo resource/key scope; cache được parse và unwrap xác thực đúng workspace/resource/client key trước tái sử dụng. Cache thiếu/hỏng/sai key thì tạo envelope hợp lệ mới qua primitive hiện hành. Regression so checksum envelope giữa hai snapshot liên tiếp.
- Không đổi schema/receipt, manifest contract, key/nonce construction, server hay crypto/KEK. Không tạo key/manifest giả, không bỏ integrity check, không xóa legacy data. Các xử lý trước mục này trong file giữ hydrate integrity/staging, immutable upload buffers, bounds, recovery markers và generation fencing.
- Không chạy test/build/QA theo chỉ thị. `git diff --check` chưa chạy sau mục này. Compile/regressions và tích hợp cuối còn chờ. Residual: API projection hiện không nhận session epoch; caller cần fence worker cũ qua reopen/session mới. Dirty retry qua restart và worker cancellation cần kiểm chứng tích hợp cuối.
- Audit retry tiếp tục phát hiện `ProxyCredentialEnvelope::seal` cũng mã hóa ngẫu nhiên, làm checksum/root đổi khi retry cùng credential. Projection nay cache envelope theo proxy/revision/resource-key scope và chỉ tái sử dụng sau khi mở bằng client key hiện tại, xác nhận workspace/resource/proxy/revision và bytes secret đúng; cache lỗi thì seal lại bằng primitive hiện hành. Regression proxy snapshot lặp lại cùng inputs và so checksum.
- Lượt đọc caller ngoài scope cho thấy desktop hiện vẫn gọi `begin_active_session`/`prepare_sync_commit` biến thể không có epoch; biến thể `*_for_epoch` trong projection chưa được wire. Vì write scope cấm `desktop_state.rs`, callback/upload cũ qua reopen chưa thể fence hoàn toàn. Không đổi authorization, lease, API/server.
- Root cause retry bổэлт: tombstone mới từng dùng wall-clock `now` mỗi snapshot nên xóa-file retry làm manifest bytes/idempotency identity đổi. Nay cache tombstone vector theo cache scope + base generation/root + descriptor root; serialize/check bounded marker, retry giữ timestamps; cache corrupt trả integrity error, hết retention/set mismatch thì recompute. Regression snapshot delete retry so generation/root/tombstones. Không đổi receipt/schema hay retention policy.
- Session 2 integration handoff update: added backward-compatible epoch-aware projection methods `begin_active_session_for_epoch`, `prepare_sync_commit_for_epoch`, and `clear_active_session_for_epoch`; old methods remain wrappers for legacy unscoped markers. Active/commit markers accept missing epoch as legacy null; epoch APIs require exact token match for begin, commit retry and clear. Added same-file regression for stale begin/prepare/clear and exact retry. Legacy null commits remain recoverable through existing recovery receipt path, but epoch-scoped retry deliberately does not accept an unscoped marker as its own commit. No tests/build run. Projection cache-reuse fixture accept timeout increased from 250ms to 5s after integration run surfaced api_unavailable under suite load; production download path unchanged.
- Re-read current caller before treating earlier residual as open: `desktop_state.rs` now calls all three epoch APIs for begin/prepare/clear, so generation/session stale-result fence is wired in the current worktree. Tombstone retry cache now wraps base generation/root and descriptor root identity plus a checksum of serialized tombstones; bounded to 64 KiB before durable write. Added source regression for corrupted cache rejection. `git diff --check` PASS; no test/build/QA per Session 2 instruction. Epoch path and new regression remain unexecuted here.
- Further static audit: marker helpers previously used `symlink_metadata` then unbounded `fs::read`, permitting a replaced/grown marker to exceed the pre-read limit. Shared marker read now opens a regular file, bounds allocation/read to 64 KiB + 1, and rechecks handle/path metadata; a present marker that changes mid-read is an integrity error, never treated as missing. `pending_session_paths` previously swallowed malformed commit/session JSON and identity fields, potentially losing recovery intent on app restart; present invalid markers now return 422 and stay on disk. Added regression for corrupt pending commit marker preservation. `git diff --check` PASS; no test/build/QA per instruction.
- Follow-up classification: bounded marker I/O now distinguishes permission-denied (actionable, non-retryable), storage-full and invalid-data from transient I/O; timeout remains retryable. Added classifier regression. No test/build/QA run.
- Recovery audit follow-up: marker I/O permission errors now use non-retryable `profile_marker_not_readable`; storage-full and invalid-data have explicit classification, transient errors stay retryable. Replaced remaining production `fs::read` call sites for active/commit/projection markers with bounded marker reader, including dirty generation check and commit/hydration fences. Snapshot envelope cache reads now propagate I/O/permission errors instead of swallowing them as cache miss; malformed/invalid envelope alone may be rebuilt after the existing unwrap/content check. `git diff --check` PASS; tests/build/QA remain prohibited and unrun.
- Snapshot cache audit: object/chunk ciphertext cache hits previously used `.ok()` for path/metadata/read, converting permission/transient I/O into cache miss and re-encrypting needlessly. Added bounded regular-file cache reader; missing/truncated/corrupt authenticated ciphertext rebuilds from current local plaintext, while permission/transient I/O surfaces with correct retry classification. Immutable cache path reuse now verifies bytes through this reader before accepting same-checksum file. No network/lease/auth/schema changes. `git diff --check` run; tests/build/QA not run.
- Bounded cache reader regression now asserts oversize cache is rejected before bytes are returned/read into a buffer. No test execution per Session 2 instruction.
- Dirty-generation classification audit: `dirty_local_generation` previously mapped every verification error to `Some(path)`, so marker corruption/permission failures looked like legitimate dirty data and could retry indefinitely. It now returns dirty only for verified local file-set/content mismatch (`profile_projection_incomplete` / `profile_projection_file_invalid`) and propagates marker/I/O failures. Generation-file permission is explicitly non-retryable; missing/truncated file remains local change to preserve. Added classification regression. `git diff --check` PASS; no tests/build/QA.
- Follow-up marker structure check: duplicate/invalid `local_objects` path/size/checksum now fails marker integrity validation before file-set comparison in verify/recovery paths. This prevents corrupted projection metadata from being labeled as a user dirty edit. Traversal and generation metadata permission failures use actionable non-retryable classification. Existing duplicate-marker regression covers recovery helper; no tests/build/QA run.
- Recovery edge audit: `local_generation_is_recoverable` previously treated an existing generation with missing or mismatched projection marker as ordinary “not recoverable”; callers then returned generic generation-missing/conflict, hiding integrity corruption. Existing generation now requires a present matching marker; missing marker returns specific non-retryable integrity error, identity mismatch returns marker-invalid. Truly absent generation remains a miss. Added regression asserting dirty file bytes remain intact. `git diff --check` PASS; no test/build/QA.
- Commit contract audit: pending-commit marker creation now validates full base and target manifest before checking parent/generation identity. This binds durable recovery intent to validated manifest root/object checksums rather than checksum-shaped strings alone. Added regression for target object checksum tampering; malformed target cannot create/replace commit marker. Caller creates marker before cloud commit and promotes/clears only after API acceptance. No lease/permission changes; no test/build/QA.
- Tightened manifest verification: manifest validation alone accepts checksum-shaped object values without recomputing the payload root. Commit preparation now reconstructs `PayloadDescriptor` for base and target, then `verify_manifest` checks canonical root and object set before the marker is written. Regression asserts checksum tampering fails `payload_root_checksum_mismatch` with no marker. Tests/build/QA remain unrun.
- Conflict-check audit: `active_session_conflicts_with_manifest` previously trusted generation/checksum fields in a present commit marker and did not validate the remote manifest checksum before comparing an active session. It now validates `base + 1 == target`, both commit checksums, and active manifest checksum; malformed state returns integrity errors instead of a false conflict result. Added same-file regression for malformed manifest and commit marker metadata. `git diff --check` PASS; no tests/build/QA per instruction. Compile and full integration remain unverified.
- Follow-up: validate incoming manifest and reconstruct/verify its canonical `PayloadDescriptor` before conflict comparison, including when a pending commit marker exists. This prevents checksum-shaped but root/object-inconsistent input from being mistaken for a committed retry/conflict. Regression covers invalid incoming checksum on both active-session and pending-commit branches. Bounded retries/deadline remain coordinator-owned per resilience plan; projection classifies outcomes without nested retries. No tests/build/QA.
- Session-clear integrity follow-up: epoch clear previously fenced only on profile ID and epoch, so a malformed state/generation/checksum could still delete active/commit recovery markers. It now rejects malformed active marker structure as non-retryable 422 before clearing; valid but different epoch remains stale 409. Extended session epoch regression to prove malformed marker bytes remain preserved. No rights/lease changes; no tests/build/QA.
- Recovery-input fence follow-up: `active_session_recovery_path` and `verified_local_recovery_receipt` could compare/advance local recovery state using an unvalidated manifest. Both now validate schema and canonical payload root/object bindings before reading recovery markers or advancing/clearing generation fences. Regression verifies invalid checksum leaves active marker intact. No tests/build/QA.
- Recovery lookup follow-up: `dirty_local_generation` and `existing_local_generation` also accepted a manifest as a path identity without validating its schema/root/object set. Both now validate before returning any local generation path; regression checks malformed checksum yields `invalid_manifest_checksum` and keeps the active marker unchanged. `git diff --check` checked; tests/build/QA remain unrun.
- Session start validation: `begin_active_session_for_epoch` accepted generation zero and could advance hydration fence before any valid manifest generation. It now rejects zero before creating profile state; epoch regression asserts no profile directory is created. Rights/lease rules untouched. Tests/build/QA not run.
- Snapshot preflight: encrypted snapshot previously walked/read/encrypted/staged profile files before final manifest validation caught an invalid prior manifest or zero lease fencing token. It now validates the previous manifest/canonical descriptor and rejects zero fencing token before source traversal, preserving the existing `invalid_manifest_fencing_token` 400 contract. Regression uses a nonexistent source to prove validation wins before scanning. No tests/build/QA.
- Recovery receipt audit: `verified_local_recovery_receipt` accepted commit targets with generation gaps and its second marker read checked less than the first. It now consistently requires `target == base + 1`, valid base/target checksums and matching projection base before exposing recovery data. Regression corrupts a pending target generation and verifies marker plus dirty bytes remain intact. No tests/build/QA.
- Restart recovery scan audit: `pending_session_paths` accepted commit generation gaps and active-session generation zero; recovery readers had the same zero-generation gap in active markers. Scans/readers now reject these marker identities before returning local recovery paths. Added pending-scan regression that preserves a gap marker. No test/build/QA.
- Snapshot source-root validation: top-level profile path previously went straight to `read_dir`; missing root was marked retryable and a symlink root could traverse outside the profile. Snapshot now requires a real directory before scan, maps a missing root to non-retryable `profile_source_missing` 409, and rejects non-directory/symlink roots. Regression verifies missing-root classification after earlier manifest/fence preflight. No tests/build/QA.
- Shared source-root guard: the transfer-memory admission estimate uses a separate full inventory scan before snapshot; it previously bypassed snapshot root validation and could treat a missing profile as retryable. Both estimation and snapshot now use the same root validator, so they reject the same missing/symlink/non-directory source before traversal. No tests/build/QA.
- Snapshot scan latency: `snapshot_memory_charge_bytes` walked and retained a full file inventory before snapshot walked it again. Snapshot processes one file at a time; small-file peak is bounded by 16 MiB × 3, large-file path uses 4 MiB chunks. Estimator now returns this conservative 48 MiB bound without scanning the tree, removing one full directory traversal and inventory allocation. Updated regression expectation for both file sizes. No tests/build/QA.
- Correction after global-budget audit: fixed 48 MiB admission overcharged small profiles and could starve unrelated hydration/downloads under the shared 128 MiB budget, increasing capacity retries. Restored measured per-profile peak estimate (3× max of small-file size or one 4 MiB chunk); the extra inventory walk remains until an in-scope API can safely pass/reuse the inventory. Updated regression to expect 12 MiB for a 4 MiB chunked file. No tests/build/QA.
- Inventory allocation follow-up: retain the measured per-file peak charge but avoid materializing a second `Vec` of every source path during admission. Snapshot and peak estimator now share the same recursive filtered visitor; estimator folds the maximum working set as entries arrive. Traversal remains one extra read pass, but no duplicate path/vector allocation and no budget overcharge. Tests/build/QA not run.
- Hydration fence consistency: record/advance/ensure paths now reject persisted fence generation zero as integrity corruption, rather than replacing it with a newer fence. Regression confirms ensure and record return non-retryable marker error and preserve malformed bytes. No test/build/QA.
- Startup staging cleanup classification: `cleanup_abandoned_staging` mapped permission/storage errors to retryable `unavailable()` and swallowed every `symlink_metadata` error as absence. Permission now returns actionable non-retryable `profile_staging_not_removable`; storage-full returns 507; only truly missing staging is ignored and transient IO remains retryable. Added classifier regression. Does not delete anything outside app-owned staging. No tests/build/QA.
- Recovery validator consistency: final audit found zero-generation active-session records still passed `active_session_conflicts_with_manifest`, commit-preparation validation, and standalone current-session validation. Those paths now reject generation zero consistently with recovery/clear/pending-scan paths, avoiding false conflict classification or commit acceptance. Same-file regression covers conflict and commit rejection. Tests/build/QA not run.
- Local persistence retry classification: expanded the staging IO classifier to active-session and pending-commit marker directory creation and atomic temp-write/sync/rename. Permission/disk-full no longer retry as outage; marker read/metadata IO also uses the marker classifier. Atomic temp+rename and generation/session fences remain unchanged. Added classification regression. No tests/build/QA.
- Projection open retry classification: projection root mkdir/canonicalize and process root-lock IO previously became retryable unavailable, including permission denied and storage full. They now use actionable local-storage classification; lock contention remains its existing non-retryable 409. Regression covers lock-contention contract. No tests/build/QA.
- Recovery scan retry classification: `pending_session_paths` previously mapped projection-root enumeration and entry metadata errors to retryable 503, including permission denied/storage full. It now reports actionable local read/storage errors and retries only transient IO. No marker, receipt, lease, or permission contract changed. No tests/build/QA.
- Recovery scan classifier regression: same-file classification coverage now asserts permission, storage-full and transient IO outcomes for projection-root scans. Static audit continues; no tests/build/QA.
- Static boundary audit: projection APIs bind hydration publication to the durable generation fence and commit preparation to active-session epoch/base; same-generation/base sessions remain indistinguishable without a caller token. Snapshot blobs provide bounded checksum-verified reads, but the upload future, its transfer permits, and network cancellation are owned by `desktop_state.rs`; this session cannot change that lifecycle under the assigned scope. The resilience plan's single retry budget and caller-owned cancellation cleanup therefore remain integration work. No tests/build/QA.
- Cache-retention audit: immutable `.sync-cache` object paths are content-addressed and never pruned by `profile_projection.rs`; repeated edits therefore accumulate old ciphertext. No safe prune point exists in this file because the upload caller still owns live `SnapshotBlob` references and commit success/retry completion is signaled outside projection. Deletion could break an in-flight/retryable upload or legacy cache recovery, so leave artifacts intact; integration owner must add reference-aware post-commit GC. No tests/build/QA.
- Snapshot latency boundary audit: `snapshot_memory_charge_bytes` and `snapshot_with_proxy_encryption` each traverse the stopped profile; the first pass computes the exact peak working-set charge required by the shared memory budget, the second builds/validates the immutable snapshot. Eliminating the duplicate walk safely requires passing one captured inventory from estimator to snapshot and revalidating its file metadata; that changes the caller contract in `desktop_state.rs`, outside this packet. Do not replace the measured charge with a fixed overcharge that can trigger capacity retries. No tests/build/QA.
- Hydration cache follow-up: verified-generation opens with an already available `ClientKey` previously downloaded the recovery-key envelope on every cache hit even though snapshot staging keeps the exact content-addressed envelope locally. Hydration now reads that bounded cache path only when its size and checksum match the signed-by-manifest reference; missing/malformed cache falls back to remote, permission/transient IO remains an error, and transfer-memory admission covers local bytes too. Recovery-key-only opens still fetch the envelope. Regression source now expects zero blob requests on the verified cached-generation path. No tests/build/QA.
- Legacy envelope-cache compatibility: hydration cache lookup now checks both the immutable object cache and the existing logical `resource-key-envelope.json` alias, accepting either only on exact manifest size/checksum. Regression fixture removes the immutable copy and proves through its request-failing remote endpoint that the legacy alias still avoids transfer. No tests/build/QA.

## QA bổ sung — Codex /root — 2026-10-28
- Build production Windows local 3.0.30 thành công tại `G:\buglogin-target\x86_64-pc-windows-msvc\release\buglogin-desktop-app.exe`; mở hai cửa sổ A/B. Production binary từ chối API fixture loopback bằng guard HTTPS; giữ nguyên guard.
- Build debug QA riêng và mở hai app A/B với SQLite/chunk fixture cô lập. API desktop login trả Bearer thành công cho Owner, Admin, Lead, Operator, Member. Member đã vào workspace UI; app QA đóng bình thường (exit 0, không có Windows crash record) trong các lượt quan sát sau auth, nên chưa coi UI A/B/lifecycle theo role là pass. Cần điều tra lý do đóng app trước khi xác nhận.
- `cargo check --locked -p buglogin-desktop-app` PASS. `desktop_state::tests`: 26/26 PASS. `profile_projection::tests`: lần đầu một test có fixture root checksum không canonical; sửa fixture `encrypted_snapshot_retry_keeps_delete_tombstone_identity`, sau đó nhóm 59/59 PASS. Build production được thực hiện trước chỉnh sửa test-only này.
- Fixture chứa QA credentials nằm dưới `%TEMP%\buglogin-local-qa-api-vuk8t8`; phiên API đã dừng nhưng tự cleanup không chạy khi Ctrl-C. Codex đã xác minh path dưới `%TEMP%`; lệnh xóa bị auto-review chặn nên chưa xóa fixture/credential temp. Không có production data trong fixture.
- Chưa release: production UI chưa thể QA bằng local API; chưa có macOS installed-app evidence theo desktop release contract; UI role/lifecycle chưa pass. Hai cửa sổ production QA A/B vẫn mở.
