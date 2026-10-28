# Platform → Organization → Workspace → Groups / Profiles → Members: UI/UX Audit Plan

Date: 2026-10-27
Status: implementation in progress; first inventory-loading improvement is local and not released.
Purpose: map the current management experience, challenge its assumptions in 30 review rounds, and implement the prioritized work in slices. This covers only this management hierarchy, not a redesign of the whole application.

## Scope and authority

Included: Platform Admin entry/context; organization inventory and detail; workspace inventory/detail; Groups and Profiles as workspace resources; organization account directory; workspace members, roles, invitations, group access, and navigation between them.

Not included: profile browser lifecycle, profile detail operational UI, general account page, Cookie Bot, Agent, extension catalog, and API redesign except where a backend contract is needed to make a UI state truthful.

Read before implementation: `docs/design/ui-authority.md`, `docs/design/information-architecture.md`, `docs/design/screen-state-matrix.md`, `docs/superpowers/specs/2026-10-26-management-ui-ux-design.md`, `docs/superpowers/specs/2026-10-24-org-workspace-management-design.md`, and `docs/agents/work-packet-ownership.md`. The Oct 26 management spec is the current layout/action authority; older DES-03 references are not design authority where they conflict. Keep Donut density, tokens, controls, keyboard behavior, and status language.

## Current implementation map

| Surface | Current source | Current behavior to review |
|---|---|---|
| Desktop route title and shell | `src/app/desktop-home.tsx` | `platformAdmin` title is a hardcoded “Bàn Điều Hành Quản Trị Nền Tảng (Super Admin)”; page context is shown outside `workspaces` and `orgCenter`. |
| Platform navigation | `src/components/management/platform.tsx` | One page called Platform Admin owns tabs for organizations, workspaces, quotas, accounts and audit. The view component does not receive an explicit acting scope. |
| Org/workspace inventory and detail | `src/components/management/org-workspaces.tsx` | One component serves personal/org and platform views, owns local route state, detects management access using platform-admin OR organization owner, and drills into selected workspace. |
| Workspace tabs and overview | `src/components/management/workspace.tsx` | Local tabs for overview, members, groups, profiles; tab state is not URL-addressable and starts at Overview after mount. |
| Workspace members/access/invitations | `src/components/management/members.tsx` | Loads members, groups, actor and group shares; permits platform admin, managed workspace IDs, or owner/admin/lead UI paths; role picker and access operations are embedded in a large page component. |
| Org account directory | `src/components/management/accounts.tsx` | Shared account directory/provisioning surface with scope-dependent actions and account vs membership concerns. |
| Org member roster | `src/components/organization-members-page.tsx` | Separate organization-level list with organization-member status and workspace assignment/invitation entry points. |
| Role labels and strings | `src/components/management/org-platform-i18n.ts`, `src/components/management/accounts-i18n.ts` | “Platform Admin” is a fixed title; role and access vocabulary is split across namespaces and surfaces. |
| Existing target behavior | `docs/superpowers/specs/2026-10-26-management-ui-ux-design.md` | Calls for explicit Org → WS navigation, clear “Your access”, separate identity from membership, single/bulk workflows, and no lifecycle controls in management inventories. |

### Verified mismatch: authority label vs active scope

When a user opens the `platformAdmin` route, `desktop-home.tsx` supplies the fixed Super Admin page title. The platform page itself has no typed “acting as Platform Admin / Organization Owner / Workspace Admin” context; `org-workspaces.tsx` uses backend actor data for permissions, but that scope distinction does not flow into the page title. Thus the data/permission scope can be correct while the shell tells an organization owner they are in Platform Admin. This is a presentation/context defect, not evidence of an ACL leak. Exact behavior should be confirmed against the affected account during implementation QA.

### Verified delete-flow gap and server rules

- Delete endpoints already exist: `organization.delete` and `workspace.delete`, called from `src/components/management/org-workspaces.tsx` and routed in `apps/desktop/src-tauri/src/remote_api.rs` to the API.
- Org delete currently has a UI dependency check for Org members and Workspaces and links the user away to remove them. API requires the Org to have no Workspaces (`organization_not_empty`) and rejects remaining non-owner Org members (`organization_has_members`).
- Workspace delete currently has no equivalent UI preflight. `management_tenancy.rs` can reject it for profiles, resources, groups/extension groups, teams, active operations or sync upload reservations; `MemberService::remove_owned_workspace_memberships` can also reject remaining non-owner/team memberships (`workspace_has_members`). The UI only discovers those constraints after attempting deletion and displays the returned error. This is the “kẹt member/WS” behavior to fix.
- Platform Admin is authorized by `WorkspaceService::authorize_organization_management`; Org owner and explicit Org-scoped capability grants can also be authorized. `org-workspaces.tsx` discoverability currently derives `canManageOrg` from only `actor.platformAdmin || organization.accountId === actor.accountId`; it does not reflect the service's full explicit-grant case. Capability-aware UI state should come from the existing permission source/contract, not a new frontend role guess.
- Platform Admin authority is not the same as silently bypassing dependency and data-safety rules. The UI should let an authorized Platform Admin complete the supported resolution workflow, while the API remains the final authority. A destructive cascade, if desired, needs a separate explicit API contract with atomicity, audit and recovery behavior; it must not be simulated as a sequence of unreviewed deletes from the UI.

### Other code-level risks to validate in visual/interaction QA

- `OrganizationManagement` keeps org/workspace selection, tab, filters and page in local component state. Deep linking, refresh preservation, browser/back behavior and returning to the same scoped context are not established by this component.
- `ManagementWorkspace` uses compact header tabs but sets the workspace name as the nav aria label; verify screen-reader context includes organization + workspace and selected tab.
- `WorkspaceMembers.reload` requests member list, groups, current account, then group shares per group. This is a serial second phase and can feel slow or leave a partially incomplete group-access view. Current code shows a generic partial error, so the affected group/permission data may not be obvious.
- Role edit, group grants, invitation creation, account provisioning and member status live in multiple surfaces. The plan must make the distinction between identity, Org membership, WS membership, role, direct group grants and effective access visible without creating duplicate sources of truth.
- A row can have an account with many workspace memberships. The user needs to know which scope an action affects before applying it; avoid global-looking controls for workspace-scoped mutations.
- The existing management design specification is broad, but no evidence yet proves each section has complete loading/empty/error/forbidden/conflict/success interaction and visual QA. Audit these states instead of presuming they exist.

## Before → after mental model

```text
CURRENT (observed code shape)                     TARGET (proposed to review/approve)
──────────────────────────────                     ──────────────────────────────────
Rail: Platform Admin                               Rail: Platform administration
  └─ one combined platform page                     ├─ Global platform scope (only when global)
       ├─ Organizations                              └─ Organization context: BugMedia
       ├─ Workspaces                                      ├─ Overview / Workspaces
       ├─ Quotas                                            ├─ Accounts & memberships
       ├─ Accounts                                          └─ Audit / Settings
       └─ Audit
                                                        Org page header:
Org Owner enters same route/page title:                    BugMedia · Your role: Owner
“Super Admin” even though backend scope is Org             [Workspaces] [People] [Audit] [Settings]

Org → Workspace → tabs are component-local:             Org → Workspace breadcrumb/context:
Back/refresh may lose exact view; role/context            BugMedia / Media Operations
may be implicit in current workspace selector.          [Overview] [Members] [Groups] [Profiles]

Member operations distributed across:                    One member access summary, grouped by scope:
Org directory, Org roster, WS Members, Groups              Identity (account) — managed separately
and Profile access; similar “role/access” words.          Organization membership — active/suspended
                                                        Workspace role — Owner/Admin/Lead/Operator/Member
                                                        Group grants — direct/inherited/effective
                                                        Action copy always names the target scope.

Counts/errors may be disconnected by request:            Scoped, paginated summaries; partial results name
slow multi-fetch and generic partial error.              which dataset failed and keep valid data usable.
```

This is a review target, not a finalized replacement design. The user will choose which proposals to implement.

## Full ASCII UI/UX proposal — actions and data flows

These screens make the proposed data actions concrete. They are discussion wireframes, not a mandate to implement every action. Every action remains subject to existing backend capability, scope and permission contracts.

### Wording guide for the UI

| Tránh dùng trên màn hình | Cách gọi dễ hiểu hơn |
|---|---|
| Danh tính / identity | Thông tin tài khoản |
| Membership | Thành viên trong… / Quyền tham gia… (ghi rõ Tổ chức hay Workspace) |
| Kế thừa / inherited | Được cấp qua nhóm/team (nêu tên nhóm hoặc team) |
| Grant | Cấp quyền truy cập |
| Quyền hiệu lực / effective access | Quyền hiện có |
| Scope | Phạm vi quản lý / Đang quản lý… |
| Global | Toàn nền tảng |
| Actor | Người đang thao tác |
| Provisioning | Tạo tài khoản |
| Quota / allocation | Hạn mức / Phân bổ hạn mức |

Các thuật ngữ ở cột trái là nhãn kỹ thuật, không đưa nguyên văn vào UI.

### 1. Platform Admin home — global scope only

```text
┌──────────────────────┬────────────────────────────────────────────────────────────────────────────────────────────┐
│ BUGLOGIN             │ Quản trị nền tảng / Tổ chức                         Phạm vi: Toàn nền tảng · Quản trị viên│
│                      ├────────────────────────────────────────────────────────────────────────────────────────────┤
│ Profiles             │ [Organizations] [Workspaces] [Accounts] [Resources & grants] [Audit] [Jobs]             │
│ Groups               │                                                                                            │
│ Members              │ Organizations                                                        [+ Create ▾] [↻]   │
│ Network              │ [Search org name / owner................................] [Status ▾] [Plan ▾]             │
│ Extensions           │                                                                                            │
│                      │ □ Organization       Owner                 Workspaces   Members   Status     Actions      │
│                      │ □ BugMedia           An Nguyen             4 / 10       18         Active     [Open] [⋯]   │
│                      │ □ Tera Media         Linh Tran             2 / 10       7          Active     [Open] [⋯]   │
│                      │ □ Win Team            Minh Vu               1 / 5        3          Active     [Open] [⋯]   │
│                      │                                                                                            │
│                      │ 3 selected  [Change plan/limits] [Export] [Clear selection]                               │
│                      │ Showing 1–25 of 81                                        [‹] 1 2 3 … 4 [›]             │
└──────────────────────┴────────────────────────────────────────────────────────────────────────────────────────────┘

Organization row ⋯: Open organization · Edit details · Transfer owner · Change platform policy · View audit
                  Delete organization (only if policy permits; show blockers before confirmation)
Bulk actions show only when the selected rows support that exact operation.
```

### 2. Organization home — owner/admin scope

```text
┌──────────────────────┬────────────────────────────────────────────────────────────────────────────────────────────┐
│ BUGLOGIN             │ Tổ chức / BugMedia                                      Quyền của bạn: Chủ sở hữu│
│                      ├────────────────────────────────────────────────────────────────────────────────────────────┤
│ Profiles             │ [Workspaces] [Accounts & memberships] [Audit] [Settings]                                 │
│ Groups               │                                                                                            │
│ Members              │ Workspaces                                      [+ Create workspace ▾] [↻]               │
│ Network              │ [Search workspace............................] [Sort: Name ▾] [Bulk select]               │
│ Extensions           │                                                                                            │
│                      │ □ Workspace             Profiles      Seats        Default extension group   Actions    │
│                      │ □ Team Anh Hòa 1        12 / 50       4 / 20       Facebook tools            [Open] [⋯]│
│                      │ □ Team Anh Hòa 2         8 / 50       3 / 20       Facebook tools            [Open] [⋯]│
│                      │ □ Team Anh Hòa 3         0 / 50       1 / 20       —                         [Open] [⋯]│
│                      │                                                                                            │
│                      │ 2 selected [Change limits] [Add accounts to workspaces] [Remove memberships] [Clear]     │
│                      │                                                                                            │
│                      │ Usage counts may load after the rows; failed counts show “Unavailable · Retry” per cell. │
└──────────────────────┴────────────────────────────────────────────────────────────────────────────────────────────┘

Workspace row ⋯: Open · Edit name/limits · Manage members · Manage groups · Manage profiles · Delete (guarded)
Create workspace ▾: One workspace · Multiple workspaces (paste table / CSV)
```

### 3. Create or edit one / many workspaces

```text
┌ Create workspaces in BugMedia ────────────────────────────────────────────────────────────────────────────────┐
│ [Single workspace] [Multiple workspaces]                                                                    × │
│                                                                                                                │
│ Single                                                                                                         │
│ Name *                         Profile limit *        Member seats *       Default extension group             │
│ [Team Anh Hòa 4.............]  [50.................]  [20...............]  [Choose group ▾]                    │
│                                                                                                                │
│ Multiple — paste rows: name | profiles | seats | default extension group                                      │
│ ┌──────────────────────────────────────────────────────────────────────────────────────────────────────────┐ │
│ │ Team Anh Hòa 4       50       20       Facebook tools                                                    │ │
│ │ Team Anh Hòa 5       50       20       Facebook tools                                                    │ │
│ └──────────────────────────────────────────────────────────────────────────────────────────────────────────┘ │
│ [Paste from Excel] [Import CSV] [Download template] [Add row]                                                │
│                                                                                                                │
│ Review: 2 ready · 0 invalid · allocated limits remain within Org allowance                                   │
│ Name duplicate / quota exceeded / unknown group are shown on the exact row; no silent row is skipped.        │
│                                         [Cancel] [Review 2 rows]                                              │
└──────────────────────────────────────────────────────────────────────────────────────────────────────────────┘

Review next: old → proposed values, total Org allocation, per-row validation and explicit commit.
Result: created/failed per row; retry failed rows only; successful rows are not re-submitted.
```

### 4. Workspace home and action data

```text
┌──────────────────────┬────────────────────────────────────────────────────────────────────────────────────────────┐
│ BUGLOGIN             │ BugMedia / Team Anh Hòa 1                         Quyền của bạn: Chủ sở hữu│
│                      ├────────────────────────────────────────────────────────────────────────────────────────────┤
│ Profiles             │ [Overview] [Members 4] [Groups 3] [Profiles 12] [Audit] [Settings]                        │
│ Groups               │                                                                                            │
│ Members              │ Overview                                                                                  │
│ Network              │ ┌ Members ──────────┐ ┌ Groups ───────────┐ ┌ Profiles ────────┐ ┌ Limits ──────────────┐ │
│ Extensions           │ │ 4 / 20 seats      │ │ 3 groups         │ │ 12 / 50         │ │ 38 profile slots left│ │
│                      │ │ [Manage members]  │ │ [Manage groups]  │ │ [Manage profiles]│ │ [Review limits]     │ │
│                      │ └───────────────────┘ └──────────────────┘ └──────────────────┘ └─────────────────────┘ │
│                      │                                                                                            │
│                      │ Recent management changes / outstanding invitations / incomplete operations               │
└──────────────────────┴────────────────────────────────────────────────────────────────────────────────────────────┘

“Profiles” here means inventory, group assignment and governance only. Launch/run/stop/sync stay in the operational Profiles page.
```

### 5. Workspace Members — searchable, selectable, scope explicit

```text
┌ BugMedia / Team Anh Hòa 1 / Members ───────────────────────────────────────────────────────────────────────────┐
│ 4 hoạt động · 1 đang mời · Quyền của bạn: Chủ sở hữu        [+ Thêm tài khoản ▾] [Mời] [Làm mới]               │
│ [Search name, username, email........................] [Role ▾] [Status ▾] [Group access ▾]                    │
│                                                                                                                │
│ □ Thành viên             Vai trò WS   Quyền nhóm                  Trạng thái         Thao tác                │
│ □ An Nguyen              Quản trị viên Tất cả · theo vai trò       Đang hoạt động    [Sửa quyền] [⋯]        │
│ □ Linh Tran              Thành viên    Facebook Ads                Đang hoạt động    [Sửa quyền] [⋯]        │
│ □ Quan Pham              Vận hành      Chưa được cấp nhóm          Đang hoạt động    [Sửa quyền] [⋯]        │
│ □ Hoa Le                  Thành viên    Đang chờ lời mời            Đang mời          [Gửi lại] [⋯]           │
│                                                                                                                │
│ Đã chọn 2 [Đổi vai trò] [Sửa quyền nhóm] [Tạm ngưng] [Gỡ khỏi Workspace] [Bỏ chọn]                           │
│                                                                                                                │
│ Thông tin tài khoản không đổi. Các thao tác này chỉ áp dụng trong Team Anh Hòa 1.                             │
└──────────────────────────────────────────────────────────────────────────────────────────────────────────────┘

Member row ⋯: View access summary · Edit workspace role · Edit group access · Suspend/Reactivate · Remove from WS
Invitation row ⋯: Copy invite link (if supported) · Resend · Revoke
No account-wide password/reset/delete actions appear here unless a distinct authorized platform flow is opened.
```

### 6. Add existing accounts vs invite vs create account

```text
┌ Thêm người vào Team Anh Hòa 1 ───────────────────────────────────────────────────────────────────────────────┐
│ [Tài khoản có sẵn] [Mời qua email] [Tạo tài khoản]                                                      × │
│                                                                                                                │
│ Tìm trong danh bạ tổ chức [...........................................................]                       │
│ □ Tên / username / email                 Thành viên của Tổ chức      Thành viên trong Workspace                │
│ □ An Nguyen · an@company.com              Đang hoạt động              Chưa tham gia                             │
│ □ Linh Tran · linh@company.com            Đang hoạt động              Đã tham gia                              │
│                                                                                                                │
│ Vai trò Workspace [Thành viên ▾]       Quyền nhóm ban đầu [Chọn nhóm ▾]                                     │
│ Xem lại: sẽ thêm 1 người · 1 người đã tham gia nên không thay đổi                                            │
│                                                                                                                │
│ [Hủy]                                                [Thêm 1 tài khoản vào Team Anh Hòa 1]                    │
└──────────────────────────────────────────────────────────────────────────────────────────────────────────────┘

Mời: nhập email → chọn vai trò và nhóm → kiểm tra trùng/lỗi → gửi lời mời.
Tạo tài khoản là luồng riêng, có bước bàn giao thông tin đăng nhập; không tự tạo khi bấm “Thêm”.
```

### 7. Member access editor — role and grants separated

```text
┌ Quyền truy cập · Linh Tran · Team Anh Hòa 1 ─────────────────────────────────────────────────────────────────┐
│ Thông tin tài khoản                  Thành viên của Tổ chức                  Thành viên trong Workspace       │
│ linh@company.com                     Đang hoạt động · BugMedia               Đang sửa quyền tại Team Anh Hòa 1 │
│ [Xem tài khoản]                      [Quản lý ở trang Tổ chức]                Phạm vi đang chỉnh sửa           │
├──────────────────────────────────────────────────────────────────────────────────────────────────────────────┤
│ Workspace role                                                                                                 │
│ ( ) Quản trị viên   ( ) Trưởng nhóm   ( ) Vận hành   (●) Thành viên                                         │
│ Thành viên dùng được các nhóm được cấp bên dưới hoặc thông qua team.                                         │
├──────────────────────────────────────────────────────────────────────────────────────────────────────────────┤
│ Group access                                                                                                   │
│ Nhóm                  Quyền hiện có       Được cấp từ                    Thay đổi                             │
│ [x] Facebook Ads       Xem + thao tác      Cấp trực tiếp                  [Xem ▾] [Thao tác ▾]                │
│ [ ] TikTok Shop        Chưa có quyền       —                              [Cấp quyền truy cập]               │
│ [x] Shared Research    Chỉ xem             Team: Research                 Cấp qua team · sửa tại Team         │
├──────────────────────────────────────────────────────────────────────────────────────────────────────────────┤
│ Thay đổi: 1 vai trò Workspace · thêm 1 quyền nhóm · giữ nguyên 1 quyền cấp qua team                            │
│                                       [Cancel] [Review changes]                                                │
└──────────────────────────────────────────────────────────────────────────────────────────────────────────────┘

Màn xác nhận ghi rõ thành viên, Workspace, vai trò cũ/mới và nơi cấp quyền. Máy chủ kiểm tra quyền lại trước khi lưu.
```

### 8. Groups inventory and group detail actions

```text
┌ BugMedia / Team Anh Hòa 1 / Groups ───────────────────────────────────────────────────────────────────────────┐
│ Groups · 3                                                   [+ Create group ▾] [Assign selected profiles] [↻]│
│ [Search group.............................................] [Member access ▾] [Sort ▾]                         │
│                                                                                                                │
│ □ Group                  Profiles   Members with access   Default for new profiles   Updated       Actions   │
│ □ Facebook Ads           8          3                     No                         Today         [Open] [⋯]│
│ □ TikTok Shop             4          2                     Yes                        Yesterday     [Open] [⋯]│
│ □ Shared Research         0          Qua team Research     Không                     3 ngày trước [Mở] [⋯] │
│                                                                                                                │
│ ⋯ Rename · Manage members/access · Set as default extension group (if supported) · Delete (show dependencies) │
└──────────────────────────────────────────────────────────────────────────────────────────────────────────────┘

┌ Group: Facebook Ads ─────────────────────────────────────────────────────────────────────────────────────────┐
│ [Profiles 8] [People & access 3] [Settings]                                                                   │
│ Profiles: search/select → [Add to group] [Remove from group]                                                  │
│ Thành viên: người · mức quyền · nơi cấp · hết hạn/thu hồi · [Cấp quyền] [Sửa] [Thu hồi]                      │
│ Trước khi xóa nhóm, hiển thị profile và quyền bị ảnh hưởng; không tự xóa dây chuyền.                           │
└──────────────────────────────────────────────────────────────────────────────────────────────────────────────┘
```

### 9. Profiles inventory — governance only

```text
┌ BugMedia / Team Anh Hòa 1 / Profiles ─────────────────────────────────────────────────────────────────────────┐
│ Profiles · 12                                                       [+ Create profile ▾] [Bulk assign] [↻]    │
│ [Search profile........................................] [Group ▾] [Access ▾] [Sort ▾]                        │
│                                                                                                                │
│ □ Name             Group(s)           Access summary       Created by       Updated          Actions          │
│ □ FB Campaign 01   Facebook Ads       3 members · 1 team    An Nguyen        Today            [Open] [⋯]      │
│ □ FB Campaign 02   Facebook Ads       3 members · 1 team    An Nguyen        Yesterday        [Open] [⋯]      │
│ □ Unsorted         —                  Owner/Admin only     Linh Tran        4 days ago       [Assign] [⋯]    │
│                                                                                                                │
│ Đã chọn 2 [Thêm vào nhóm] [Chuyển nhóm] [Xem quyền hiện có] [Bỏ chọn]                                       │
└──────────────────────────────────────────────────────────────────────────────────────────────────────────────┘

⋯ Rename metadata · Assign group · Review access · Remove from group (does not delete profile) · Delete (guarded)
Never show lifecycle controls on this screen; “Open” means inspect governance detail, not launch browser.
```

### 10. Account directory — identity vs Org/WS memberships

```text
┌ BugMedia / Tài khoản & thành viên ────────────────────────────────────────────────────────────────────────────┐
│ 18 tài khoản · 22 lượt tham gia Workspace             [+ Tạo tài khoản ▾] [Xuất] [Làm mới]                     │
│ [Search name / username / email................] [Workspace ▾] [Org status ▾] [WS status ▾]                  │
│                                                                                                                │
│ □ Tài khoản                  Trong Tổ chức    Tham gia Workspace               Quyền của bạn     Thao tác    │
│ □ An Nguyen                  Active            Team Anh Hòa 1 · Owner           Owner              [Open] [⋯]│
│ □ Linh Tran                  Active            Team Anh Hòa 1 · Member           Owner              [Open] [⋯]│
│ □ Quan Pham                  Suspended         Team Anh Hòa 2 · Operator         Owner              [Open] [⋯]│
│                                                                                                                │
│ ⋯ Xem tài khoản · Sửa thành viên Tổ chức · Chọn Workspace · Tác vụ tài khoản (nếu được phép)                  │
│ Chọn nhiều: [Thêm vào Workspace] [Đổi vai trò] [Tạm ngưng trong Tổ chức] [Gỡ khỏi Workspace]                 │
└──────────────────────────────────────────────────────────────────────────────────────────────────────────────┘

Mở tài khoản sẽ hiện thông tin tài khoản và nơi người đó tham gia. Sửa quyền tham gia không làm đổi thông tin tài khoản.
```

### 11. Edit workspace limits and default configuration

```text
┌ Workspace settings · Team Anh Hòa 1 ─────────────────────────────────────────────────────────────────────────┐
│ Limits and defaults                                                                                           │
│ Profile slots        Current 50   Used 12   Proposed [60]       Org allowance after change: 140 / 200        │
│ Member seats         Current 20   Used 4    Proposed [20]       Org allowance after change: 55 / 80          │
│ Default extension group [Facebook tools ▾]   Applies to newly created profiles only                          │
│                                                                                                                │
│ [Apply one setting to selected workspaces] [Copy settings to…]                                                 │
│                                                                                                                │
│ Validation: cannot lower below usage; no automatic increase to Org quota; server revalidates current revision.│
│                                           [Cancel] [Review changes]                                           │
└──────────────────────────────────────────────────────────────────────────────────────────────────────────────┘
```

### 12. Delete / transfer / suspend — impact before commit

```text
┌ Delete organization “BugMedia” ───────────────────────────────────────────────────────────────────────────────┐
│ This action affects the whole organization. It does not delete shared account identities.                     │
│                                                                                                                │
│ Current dependencies (from server):                                                                           │
│ • 4 workspaces                                                                                                 │
│ • 18 Org memberships                                                                                            │
│ • 22 workspace memberships · 37 groups · 104 profiles                                                        │
│                                                                                                                │
│ Blocked: resolve listed dependencies first. [Open workspaces] [Open memberships] [Cancel]                      │
│                                                                                                                │
│ When eligible: type BugMedia to confirm [________________] [Delete organization]                              │
└──────────────────────────────────────────────────────────────────────────────────────────────────────────────┘

Transfer Owner is a separate reviewed workflow: current owner → eligible new owner, impact on access, confirmation,
success receipt and audit entry. Suspend Org membership is not account-wide suspension and says so before commit.
```

### Deletion must guide the user through blockers instead of trapping them

```text
┌ Xóa Workspace · Team Anh Hòa 1 · BugMedia ────────────────────────────────────────────────────────────────────┐
│ Người đang thao tác: Platform Admin · Phạm vi: Team Anh Hòa 1                                                │
│ Kiểm tra dữ liệu mới nhất từ máy chủ                                                                          │
│                                                                                                                │
│ CẦN XỬ LÝ TRƯỚC KHI XÓA                                                                                       │
│ Profiles                  12   [Chọn nơi chuyển] [Chọn profile cần xóa]                                      │
│ Thành viên trong WS        4    [Gỡ 4 người khỏi Workspace]                                                   │
│ Nhóm và quyền              3    [Xem nhóm / quyền bị ảnh hưởng]                                               │
│ Tài nguyên                 2    [Chuyển hoặc gỡ tài nguyên]                                                    │
│ Team                       1    [Chuyển thành viên / xóa Team]                                                 │
│ Tác vụ đang chạy           1    [Mở tác vụ]   Chưa thể xóa khi tác vụ còn hoạt động                           │
│ Tải đồng bộ đang chờ       1    [Xem trạng thái] Chờ hoàn tất hoặc hết hạn theo chính sách                    │
│                                                                                                                │
│ [Làm mới kiểm tra]                                                                                             │
│ Tiến độ xử lý: 2 / 7 mục đã giải quyết                                                                         │
│                                      [Đóng] [Tiếp tục xử lý]                                                   │
└──────────────────────────────────────────────────────────────────────────────────────────────────────────────┘

Sau mỗi bước, cập nhật đúng số liệu và kết quả từ server. Khi tất cả điều kiện đạt:

┌ Xác nhận xóa Workspace ───────────────────────────────────────────────────────────────────────────────────────┐
│ Workspace: Team Anh Hòa 1 · Tổ chức: BugMedia                                                                 │
│ Thành viên sẽ mất quyền tham gia Workspace này. Tài khoản của họ vẫn tồn tại.                                 │
│ Profile/tài nguyên đã chuyển hoặc đã xóa theo lựa chọn ở trên.                                                │
│ Nhập Team Anh Hòa 1 [________________________]                                                                │
│                                               [Hủy] [Xóa Workspace]                                           │
└──────────────────────────────────────────────────────────────────────────────────────────────────────────────┘
```

Các nút xử lý phải gọi luồng thật của từng loại dữ liệu và ghi audit. Không dùng một nút “force delete” để âm thầm xóa nội dung. Nếu người dùng được phép chọn xóa hàng loạt dữ liệu con, trước khi thực hiện phải có màn xem lại đầy đủ đối tượng, kết quả dự kiến, khả năng khôi phục và một xác nhận có chủ đích.

```text
┌ Xóa tổ chức · BugMedia ───────────────────────────────────────────────────────────────────────────────────────┐
│ Người đang thao tác: Platform Admin · Phạm vi: toàn bộ BugMedia                                               │
│                                                                                                                │
│ Tổ chức còn 3 Workspace. Cần xử lý từng Workspace trước khi xóa Tổ chức.                                      │
│ Workspace              Profiles  Thành viên  Nhóm/tài nguyên  Tác vụ   Trạng thái       Thao tác             │
│ Team Anh Hòa 1          12        4           5                1        Cần xử lý       [Mở xử lý]           │
│ Team Anh Hòa 2           8        3           2                0        Cần xử lý       [Mở xử lý]           │
│ Team Anh Hòa 3           0        1           0                0        Sẵn sàng        [Xóa Workspace]      │
│                                                                                                                │
│ Thành viên Tổ chức còn 2 người: [Chọn người] [Gỡ khỏi Tổ chức]                                                 │
│ Tiến độ: 1 / 3 Workspace đã xóa · Tổ chức vẫn được giữ lại cho tới bước cuối                                  │
│                                                          [Hủy] [Tiếp tục]                                    │
└──────────────────────────────────────────────────────────────────────────────────────────────────────────────┘

Khi mọi Workspace và thành viên đã được xử lý, bước cuối hiển thị tác động, yêu cầu nhập “BugMedia”,
rồi mới gọi organization.delete với revision mới nhất. Nếu bất kỳ bước nào lỗi, giữ Org và các mục còn lại;
hiển thị mục lỗi, không báo thành công toàn bộ.
```

### Delete-flow logic to plan across UI and API

1. **Preflight:** provide one authoritative, revision-bound dependency summary for Org/Workspace, not a UI guess assembled from incomplete lists. Include counts and stable display names for blockers, safe resolution routes, active work and pending sync operations.
2. **Permission:** return the actor's actual capabilities for this target (including Platform Admin and explicit Org-scoped grants). Show “Platform Admin · Toàn nền tảng” or “Owner · BugMedia”; do not display global admin for an Org owner. API re-authorizes each resolution and final delete.
3. **Resolution matrix:** profiles can be moved using the existing atomic transfer contract when eligible, or explicitly deleted; members can be removed from this Workspace (not deleted as accounts) or moved where supported; groups/teams/resources/grants use their own lifecycle flows; active jobs and sync reservations must finish/cancel/expire under their own contracts.
4. **No false blockers:** distinguish blockers that prevent deletion from data that will be detached/removed as part of the documented delete contract. Explain consequences before cleanup.
5. **Resumable cleanup:** each resolution has an idempotency key, per-item result and audit entry. If step 4 of 7 fails, preserve prior successes and resume from remaining items. Never represent a multi-request cleanup as atomic unless the API provides that guarantee.
6. **Final delete:** reload target revision and dependency summary immediately before commit. A changed dependency returns a structured conflict and refreshes the checklist; never silently proceed with stale counts.
7. **Error mapping:** map `organization_not_empty`, `organization_has_members`, `workspace_has_profiles`, `workspace_has_resources`, `workspace_has_groups`, `workspace_has_teams`, `workspace_has_active_operations`, `workspace_has_sync_reservations`, `workspace_has_members`, revision conflict, 403 and 404 to actionable, localized UI states. Do not expose raw codes as the only diagnosis.
8. **Platform Admin:** provide the same guided operation across scopes and cross-Org search; platform authority must remove navigation friction, not weaken safety invariants. A forced cascade is a separate product/security decision and contract, not assumed in this plan.

### Create/Edit detail actions — ensure every visible action has a complete path

- Organization detail: edit name; show owner and transfer owner separately; inspect workspaces, members and audit; delete via the guided Org cleanup flow. Platform-only policy fields must be labeled and absent for actors who cannot change them.
- Workspace detail: edit name, limits and default extension group with current usage and parent allowance; manage members/groups/profiles; inspect audit; delete via the guided Workspace cleanup flow.
- Group detail: edit name/description only if persisted; manage which profiles belong to it; add/revoke member access with source and level; remove the group without accidentally deleting profiles.
- Profile governance detail: edit only supported metadata; add/remove groups and view who can use it; deletion is distinct from removing it from a group and displays its consequences.
- Member detail: show account information read-only where appropriate, Org status, Workspace role and group access; each Edit button opens the editor for that exact layer. A Platform Admin account action must not be mixed into a Workspace member edit form.
- Every Edit loads the latest record/revision; errors prevent save and offer retry. Save reports the fields changed. If revision changed concurrently, show server values and ask the user to review before retrying.

### 13. Data action result — never a generic success toast for batch work

```text
┌ Results · Add 5 members to Team Anh Hòa 1 ────────────────────────────────────────────────────────────────────┐
│ 3 succeeded · 1 unchanged · 1 failed                                                        [Download report] │
│ Person                 Result       Detail                                      Next action                 │
│ An Nguyen              Added        Member · Facebook Ads                      —                            │
│ Linh Tran              Unchanged    Already a member                           —                            │
│ Quan Pham              Added        Operator · no group grant                   —                            │
│ Hoa Le                  Failed       Invitation is expired                       [Resend invite]             │
│ Nam Do                  Added        Member · TikTok Shop                        —                            │
│                                                                                                                │
│ Succeeded rows stay committed. Retry affects only Hoa Le.                                      [Close]        │
└──────────────────────────────────────────────────────────────────────────────────────────────────────────────┘
```

### 14. Loading, partial failure and forbidden states

```text
Loading: render table-shaped skeleton for primary records; do not wait on optional usage/access summaries.
Empty: “No workspaces in BugMedia” + [Create workspace] only if caller can create.
No results: “No matches” + [Clear filters].
Partial: “Members loaded; group access failed” + [Retry group access]; retain member list.
Forbidden: name the denied scope (Org/WS) and explain who can grant access; never show blank controls.
Conflict: explain which row changed since load; refresh that row and let the user review before resubmitting.
Saving: disable only conflicting controls and show pending state beside the action; preserve the rest of the page.
```

### Data action rules applying to every screen

1. The button, confirmation, result and audit event name the exact target scope and record.
2. Separate account identity, Org membership, WS membership, workspace role, team membership, group grant and profile assignment; do not combine them under a vague “role” field.
3. Read state comes from the authoritative API. UI capability hints improve clarity but never grant permission.
4. For mutations use the existing revision/idempotency patterns. Show before → after and re-check server permissions at commit.
5. Bulk operations preview valid/invalid/unchanged rows and limits; return per-row outcomes; retry only failed eligible rows.
6. Dangerous delete/suspend/transfer operations show dependencies, blast radius, recoverability and exact confirmation. No silent cascade.
7. Preserve successful/loaded data if a sibling request fails; identify the stale/missing slice rather than showing a global spinner/error.
8. No fake counts or derived totals presented as authoritative. Label cached/estimated data and its freshness if the contract permits it.

## Focused debate: assignment flows and Create/Edit dialogs

The current implementation has real pieces, but the same-looking modal is asked to perform several different jobs. The goal is not “more modal UI”; it is fewer ambiguous choices and a clear preview of exactly which records change.

### Findings from the existing forms

- `src/components/group-assignment-dialog.tsx` is a legacy profile-to-group dialog: it selects one destination (or unassigned), gives little information about current groups for each selected profile, and opens `CreateGroupDialog` as a second dialog over the first. This makes “assign”, “move”, and “remove from group” feel like one operation even though the data effect differs.
- `src/components/management/resources.tsx` has a broad action dialog covering create, rename, assignment, access grants and delete. It includes `add/remove/replace` assignment modes, but the selection preview and meaning of each mode need to be obvious before commit; the group detail dialog also currently describes access with the technical label `inherited`.
- `src/components/management/members.tsx` combines role edits and optional group replacement in one dialog. Its checkbox says “replace groups”; that is easy to misread as adding selected groups. It builds multiple role/grant commands and then a confirmation step, so the preview should show per-person deltas, not only a count.
- `OrganizationEditor` and `WorkspaceEditor` in `src/components/management/org-workspaces.tsx` load authoritative records for edit, but the Org form mixes Org name and platform-only workspace limit; the Workspace form mixes name, profile limit, member seats and default extension group. The UI should show which fields are editable in this scope and what each limit affects.
- `src/components/management/org-workspaces.tsx` also has separate bulk workspace create/quota flows; retain those as reviewable table workflows rather than forcing many rows into a small single-record dialog.

### Assignment pattern — add / replace / remove as explicit intent

```text
┌ Gán nhóm cho 3 profile · BugMedia / Team Anh Hòa 1 ─────────────────────────────────────────┐
│ Nhóm hiện tại của các profile đã chọn                                                       │
│ FB Campaign 01  Facebook Ads                                                               │
│ FB Campaign 02  Facebook Ads, Research                                                     │
│ FB Campaign 03  Chưa có nhóm                                                              │
├────────────────────────────────────────────────────────────────────────────────────────────┤
│ Bạn muốn thay đổi thế nào?                                                                 │
│ (●) Thêm nhóm đã chọn, giữ nguyên các nhóm khác                                            │
│ ( ) Thay toàn bộ nhóm hiện tại bằng nhóm đã chọn                                           │
│ ( ) Gỡ các nhóm đã chọn khỏi profile                                                       │
├────────────────────────────────────────────────────────────────────────────────────────────┤
│ Nhóm cần thêm / thay / gỡ                                                                  │
│ [Tìm nhóm................................................]                                 │
│ [Facebook Ads ×] [TikTok Shop ×] [+ Thêm nhóm]                                            │
│                                                                                             │
│ Kết quả dự kiến: 3 profile thay đổi · 1 profile không đổi · 0 profile lỗi                 │
│                              [Hủy] [Xem lại 3 thay đổi]                                   │
└────────────────────────────────────────────────────────────────────────────────────────────┘

Review shows old groups → new groups per profile, including “unchanged”; confirm button names action and count.
If destination creation is offered, close the picker, create the group, return with that new group selected.
Do not stack two dialogs. The batch action reports per-profile success/failure and supports retrying failed rows only.
```

### Member group-access pattern — add/revoke instead of accidental replacement

```text
┌ Quyền nhóm · 2 thành viên · Team Anh Hòa 1 ─────────────────────────────────────────────────┐
│ Chọn cách cập nhật                                                                           │
│ (●) Cấp thêm quyền — giữ các quyền hiện có                                                  │
│ ( ) Thay danh sách quyền trực tiếp — quyền qua team vẫn giữ nguyên                           │
│ ( ) Thu hồi quyền đã chọn                                                                   │
│                                                                                             │
│ Chọn nhóm                                                                                   │
│ [Tìm nhóm.........................] [Facebook Ads ×] [+ Thêm nhóm]                         │
│                                                                                             │
│ Thành viên      Quyền hiện có       Sau khi lưu                                             │
│ Linh Tran       Facebook Ads        Không đổi (đã có quyền)                                 │
│ Quan Pham       Chưa có quyền       Facebook Ads · Xem + thao tác                            │
│                                                                                             │
│ Tác động: cấp mới cho 1 người · 1 người không đổi                                           │
│                                         [Hủy] [Lưu quyền]                                   │
└─────────────────────────────────────────────────────────────────────────────────────────────┘
```

Do not call a whole-list replacement “edit groups” without warning. Distinguish direct rights from rights received through a team, and never let a direct-revoke action remove a team-level right.

### Create/Edit pattern — one clear object, correct fields, honest permission

```text
┌ Sửa Workspace · Team Anh Hòa 1 ─────────────────────────────────────────────────────────────┐
│ Đang sửa trong tổ chức BugMedia                                         Quyền của bạn: Owner│
│                                                                                             │
│ Thông tin                                                                                  │
│ Tên Workspace * [Team Anh Hòa 1.........................................................]  │
│                                                                                             │
│ Hạn mức                                                                                    │
│ Profiles tối đa     [50]   Đang dùng 12                                                    │
│ Thành viên tối đa   [20]   Đang dùng 4                                                     │
│ Nhóm tiện ích cho profile mới [Facebook tools ▾]                                           │
│                                                                                             │
│ Giới hạn do tổ chức cấp: 200 profiles · 80 chỗ thành viên. Máy chủ sẽ kiểm tra lại khi lưu. │
│ Lưu ý: đổi nhóm mặc định chỉ ảnh hưởng profile tạo mới, không sửa profile hiện có.          │
│                                                                                             │
│ [Hủy]                                                   [Xem lại thay đổi]                 │
└─────────────────────────────────────────────────────────────────────────────────────────────┘
```

Create uses the same fields with empty values and the parent Org stated. Edit preloads current values; loading failure blocks save rather than presenting guessed defaults. Only show controls the current actor can actually change. If only the name is editable, do not show disabled quota inputs without a reason.

```text
┌ Xem lại thay đổi Workspace ─────────────────────────────────────────────────────────────────┐
│ Tên                   Team Anh Hòa 1 → Team Anh Hòa 1A                                     │
│ Hạn mức profile       50 → 60 · Org còn 140 / 200                                          │
│ Hạn mức thành viên    20 → 20 · Không đổi                                                 │
│ Nhóm mặc định         Facebook tools → Facebook tools · Không đổi                          │
│                                                                                             │
│ 1 trường đổi · 3 trường giữ nguyên                                                         │
│                                        [Quay lại sửa] [Lưu thay đổi]                       │
└─────────────────────────────────────────────────────────────────────────────────────────────┘
```

### Batch Create/Edit pattern — spreadsheet is the working surface

```text
┌ Tạo / sửa nhiều tài khoản trong BugMedia ───────────────────────────────────────────────────┐
│ 1. Nhập dữ liệu  ───  2. Kiểm tra  ───  3. Xem lại  ───  4. Kết quả                       │
│ [Dán Excel/CSV] [Tải mẫu] [Thêm dòng]                                                       │
│                                                                                             │
│ Tài khoản         Workspace          Vai trò WS       Nhóm       Kết quả kiểm tra            │
│ linh@...          Anh Hòa 1           Member          FB Ads     Hợp lệ                       │
│ quan@...          Anh Hòa 2           Operator        —          Đã tham gia · không đổi      │
│ bad@@...          Anh Hòa 1           Member          —          Email không hợp lệ           │
│                                                                                             │
│ 1 sẽ thay đổi · 1 không đổi · 1 cần sửa                                                    │
│ Dòng trống = giữ nguyên. Muốn xóa dữ liệu phải chọn “Xóa” rõ ràng.                         │
│                                      [Hủy] [Chỉ tiếp tục với dòng hợp lệ]                  │
└─────────────────────────────────────────────────────────────────────────────────────────────┘
```

Rows are not silently discarded; duplicate accounts resolve to an existing account or are marked invalid. “Apply to all” previews affected rows and supports undo only if the backend offers a real reversible operation.

### Create/Edit/Delete modal debate — 15 rounds

| Round | Challenge / counterargument | Outcome for the plan |
|---:|---|---|
| 1 | One generic modal component reduces code. | Reuse primitives, but keep object-specific forms and labels. |
| 2 | Put every possible field in the dialog and disable unauthorized ones. | Show only editable fields; explain hidden/non-editable policy separately. |
| 3 | Create and Edit should look identical. | Same structure is good; Create has empty input, Edit has loaded data and current values. |
| 4 | Edit can use initial list-row values without fetching the record. | Fetch the authoritative revision; show load failure and block stale save. |
| 5 | Save directly to minimize clicks. | Low-risk single field may save directly; multi-field limits/permission changes need review when blast radius matters. |
| 6 | Every form should have a review step. | Avoid needless friction; require review for bulk, quota, role, access, transfer and destructive changes. |
| 7 | A plus icon inside a picker is enough to create a missing group. | If creating inline, preserve parent context and preselect the new item; no stacked dialogs. |
| 8 | “Replace groups” is a concise technical label. | Use “Thay toàn bộ nhóm đang chọn”; disclose effect and retain team-derived rights. |
| 9 | One destination is enough for group assignment. | Multi-group membership is supported by existing management flow; show add/replace/remove intent explicitly. |
| 10 | Current assignments clutter a simple picker. | For batch edits, old → new per affected profile prevents accidental removal; show compactly. |
| 11 | Empty selection can mean remove all. | Dangerous ambiguity. Require explicit “Gỡ nhóm đã chọn” and never infer remove-all from blank. |
| 12 | Generic “Save” works for all modals. | Primary button names the consequence: “Thêm 4 thành viên”, “Cấp quyền”, “Xóa Workspace”. |
| 13 | One success toast is enough. | Fine for one simple save; batch/partial operations need result table and retryable subset. |
| 14 | Closing during save should dismiss as usual. | Prevent accidental duplicate/lost operation; keep dialog open while pending and report if outcome is uncertain. |
| 15 | Use the same confirmation for all deletes. | Confirmation must identify the object, scope, dependencies, and recoverability; exact-name confirmation only for high-impact deletion. |

### Acceptance for assignment and modal interactions

- [x] Every picker shows the parent Org/Workspace and selected record count (picker shows server-derived scope names and the selected batch size; a new-object form has no selected count).
- [x] Assignment modes use plain-language intent and preview current → resulting groups/rights for profile group assignment and direct member group grants.
- [x] “Add” preserves existing assignments; “Replace” is explicit; “Remove” only removes selected rights/groups. Member Remove starts with an empty selection to prevent accidental full revocation.
- [x] Current direct group assignments remain distinguishable from team membership; teams currently organize workspace members and do not grant group access.
- [ ] Create/Edit dialogs use authoritative loaded data, field-level validation and scope-specific permissions.
- [ ] Nested dialogs are avoided; inline create returns to the original action with the new item selected.
- [ ] Single operations have truthful pending/success/error outcomes; batch operations show per-row results and safe retry.
- [ ] Delete/transfer/suspend modals disclose impact and dependencies before the final commit.

Implementation follow-up — 2026-10-28:

- The organization directory previously exposed “Create organization” to every actor even though the backend only authorizes `CAP-ORG-001:create` for Platform Admin. The action is now shown only when the authoritative account view says `platformAdmin`; the empty-state action follows the same rule. Organization owners retain the existing scoped workspace-create action. This closes the create-action permission mismatch; the broader Create/Edit acceptance item remains open pending visual and keyboard QA plus review of the other scope-specific fields.
- Editing a workspace previously used `Promise.all` for the authoritative workspace record and the optional extension-group picker catalog. A transient extension-group list failure therefore discarded access to an otherwise valid workspace edit and left Save blocked. These reads now settle independently: failure to load the required workspace record still blocks stale edits, while failure to load the optional extension-group catalog keeps the revision-backed fields usable, preserves the stored default-group value, disables only that picker, and offers a retry.
- The form continues to submit the authoritative loaded workspace revision and existing policy fields; no partial local value is written for the unavailable picker.
- `pnpm --dir apps/desktop typecheck`, focused Biome for `org-workspaces.tsx` and `org-platform-i18n.ts`, and `git diff --check` pass. Screenshot/keyboard interaction QA and the other unchecked acceptance items remain open.
- Profile group assignment no longer opens a second Create Group dialog over the assignment dialog. Creating a group now happens inline, selects the newly created group as the destination, and preserves the assignment context. The pending create operation prevents closing the dialog or submitting the assignment; a failed create stays in the same view with a retryable error. Desktop tests/typecheck and focused Biome pass. This closes the nested-dialog issue for profile group assignment only; other management create/edit dialogs and authenticated visual/keyboard QA remain open.

## 30-round debate log

Each round challenges the current design from an operator and user perspective, then records a provisional outcome. “P” means include in plan for a later design decision; “V” means verify before changing behavior.

| Round | Challenge / counterargument | Provisional outcome |
|---:|---|---|
| 1 | “Platform Admin” is technically the route name. | Route implementation detail should not label an Org Owner as platform authority. P: context-aware title. |
| 2 | The user can see the correct organizations, so the title is harmless. | Correct data does not remove false authority cues. P: show active scope and effective role separately. |
| 3 | Rename the entire route to “Organization management.” | Platform operators still need genuine cross-tenant tools. P: separate global and tenant contexts, don’t erase either. |
| 4 | Add a role badge to every page. | Repeating badges creates noise. P: one persistent scope/role breadcrumb, details at action points. |
| 5 | Hide Platform Admin for Org Owners. | Hiding may be valid if they lack platform capability, but API authority is decisive. V: route visibility and backend permission mapping. |
| 6 | Show “Owner” as the same thing as “Platform Admin” for convenience. | Incorrect semantics can cause dangerous actions. P: never conflate org ownership with global admin. |
| 7 | Keep one combined navigation since all data is management. | Combined page can blur scope. P: distinguish platform directory from selected-org workspace. |
| 8 | Add a context switcher in the header. | Conflicts with Oct 26 decision: one breadcrumb, sidebar switcher, no duplicate header switcher. P: keep single context source. |
| 9 | Always show Org → WS → Group/Profile as a tree in sidebar. | Dense tree is awkward with many resources and duplicates current workspace switcher. V: test hierarchy navigation without a second shell. |
| 10 | Local tabs are simple and quick. | Refresh/back/deep links become unreliable. P: URL-addressable selected Org, WS and tab where router supports it. |
| 11 | Back button is enough. | It only works within current component lifetime. P: preserve selection/filter/page and browser history. |
| 12 | Workspace Overview can remain the default every time. | Re-entering a workspace forces extra clicks. V: remember last section per workspace vs always land on overview. |
| 13 | Org directory and WS members are both just member lists. | They represent different records and permissions. P: label “Accounts” vs “Workspace members” and state mutation scope. |
| 14 | Put all roles in one role dropdown. | Org membership status, WS role and resource grant are separate concepts. P: separate workflows and explain inheritance. |
| 15 | Owner/Admin automatically sees all groups; show “All groups.” | Existing UI implies inherited access; backend semantics must confirm. V: show effective access source, not a guessed grant. |
| 16 | Group/Profile pages can be generic resource tables. | They need different fields/actions; Profiles here are governance metadata, not lifecycle. P: resource-specific columns, actions and empty states. |
| 17 | Put Profile launch/run controls in management for convenience. | Contradicts UI authority: management is governance, desktop Profiles own lifecycle. P: keep boundaries clear. |
| 18 | Use one generic “Manage” action on every row. | Ambiguous when Org, WS, member, group and profile actions differ. P: one direct primary action plus scoped overflow actions. |
| 19 | Bulk actions can appear whenever checkboxes exist. | Selection without valid batch actions adds clutter/risk. P: checkboxes only when batch operations are supported. |
| 20 | Bulk member edits can default blank fields to empty. | Could erase data accidentally. P: blank means unchanged; explicit clear action; preview before commit. |
| 21 | Show workspace role in the Org account list. | Without chosen WS this risks implying a global role. P: show memberships as scoped chips/count with role per WS on expansion. |
| 22 | A successful toast is enough after partial bulk failure. | User cannot tell which members changed. P: row-level results, retry failed subset, preserve succeeded rows. |
| 23 | One generic error banner is concise. | It hides whether profiles, members, groups or grants failed. P: name the failed dataset/action and offer scoped retry. |
| 24 | Keep successful data visible while one resource request fails. | Better availability if freshness is explicit. P: retain good results, mark partial/stale sections individually. |
| 25 | Load group grants with one request per group for simplicity. | Request count and serial latency grow with group count. P: use a scoped bulk/effective-access endpoint if existing API contract allows; otherwise bounded parallelism and measurement. |
| 26 | Fetch all workspace/profile totals before rendering the list. | Blocks first paint and can create long skeletons. P: show inventory first, lazy-load secondary usage and disclose pending counts. |
| 27 | Role-based UI hiding is sufficient. | Not an authorization boundary. P: UI reflects capabilities but server rechecks every mutation. |
| 28 | IDs are stable, so they are fine as fallback labels. | Violates UI identity authority. P: verified display name/email or localized unavailable label; IDs only diagnostics. |
| 29 | Desktop-only UI can ignore accessibility and narrow windows. | Dense management needs keyboard and resized-window support. P: focus order, labels, selection semantics, responsive action menus, long-label tests. |
| 30 | Make all proposed changes in one “full management redesign.” | Scope is too wide and risks mixing hierarchy, permissions and bulk workflows. P: split into context/navigation, Org/WS inventory, memberships/access, groups/profiles, then visual QA; user approves priorities before coding. |

## Proposed plan for joint prioritization

### P0 — eliminate scope confusion

- Replace hardcoded route titles with scope-aware context derived from the authenticated actor and selected Org/WS. Keep “Platform Admin” only when the actor is acting in platform scope; an Org Owner should see the Org name and “Owner” access label. Do not infer role from route name.
- Make platform-wide actions visually distinct and gated by backend-derived platform capability. Display “Your access” at Org and WS rows/details. Never claim platform rights from Org ownership.
- Code targets: `src/app/desktop-home.tsx`; `src/components/management/platform.tsx`; `src/components/management/org-workspaces.tsx`; translations in `src/components/management/org-platform-i18n.ts`.
- Make capability discovery match the backend's Org owner, Platform Admin and explicit Org-scoped grant authorization. Do not rely on `platformAdmin || ownerId` in the UI as the complete permission model.

### P1 — make hierarchy navigable and recoverable

- Give Org and WS context consistent breadcrumbs, selected scope, and clear “back to Org / list” navigation.
- Persist selected Org, WS, subview, filters and pagination in URL/history when supported; otherwise preserve state predictably for in-app navigation and document limitation.
- Separate platform-global inventory from Org-scoped tabs so an Org Owner never mistakes the current page for global control.
- Targets: `src/components/management/platform.tsx`, `org-workspaces.tsx`, `workspace.tsx`, `src/app/desktop-home.tsx`, route utilities/router if existing.

### P0 — make Org/Workspace deletion completable with guided resolution

- Replace the current Org-only count check and Workspace post-failure discovery with an API-backed preflight listing all blockers, counts, affected records and resolutions.
- Add a resumable cleanup wizard: resolve each Workspace first, then Org memberships, then final Org delete. Resolve Workspace profiles/resources/groups/teams/members and wait for active operations or pending sync reservations through their owning workflows.
- Use existing `organization.delete` / `workspace.delete` commands after cleanup, with latest revision and server re-authorization. Keep successful cleanup steps and let the operator resume only unresolved steps after partial failure.
- No silent cascade. A destructive bulk cascade requires an explicit new security/data-retention decision and atomic API contract; do not approximate it by firing sequential deletes.
- Code targets: `src/components/management/org-workspaces.tsx` (current `DeleteTarget` and `DeleteTenancy`); `src/components/management/org-platform-i18n.ts`; `apps/desktop/src/bridge.ts`; `apps/desktop/src-tauri/src/remote_api.rs`; `crates/api/src/http/control/management_tenancy.rs`; `crates/api/src/services/workspaces/mod.rs`; `crates/api/src/services/members/mod.rs`; `crates/api/src/services/organization_members.rs`; command registry/schema/generated-client path if adding a preflight command.
- Add typed localized handling for all existing dependency errors and revision/auth failures. Platform Admin should be able to reach every resolution permitted by backend authority without needing to guess the blocker.

### P1 — clarify account, membership and access graph

- In directory/roster/detail, label account identity, organization membership/status, workspace membership/role, group grants, and effective access as distinct rows or sections.
- State the target Org/WS in dialog title, review, confirmation and success result. Explain inherited access and show its source. Prevent ambiguous “edit account” copy when the action only changes membership.
- Unify single and bulk member workflows around the existing Oct 26 spec: preview, per-row validation/result, partial success, safe retry. Keep invitation, account creation, membership assignment and role change explicit.
- Targets: `src/components/management/accounts.tsx`, `members.tsx`, `src/components/organization-members-page.tsx`, `src/components/org-member-access.ts`, i18n files, and existing command DTOs only if a missing authority field is proven.

### P2 — make Groups and Profiles governance screens understandable

- Align Group and Profile inventory columns/actions to scope and purpose. Show member access/effective grants for Groups and group assignment/access summary for Profiles. Keep launch/stop/sync out of this management surface.
- Validate whether GR/PR here means the current Groups and Profiles tabs in `ManagementWorkspace` or includes other group/profile administration surfaces; avoid duplicating operational Profile page.
- Targets: `src/components/management/workspace.tsx`, `src/components/management/resources.tsx`, related group/share components and labels.

### P1 — reduce loading ambiguity and request fan-out

- Render primary Org/WS inventory before optional usage counts. Load secondary counts without blocking and expose per-section stale/error states.
- Replace serial per-group share reads in `WorkspaceMembers.reload` with the smallest contract-backed bulk/effective-access read; if unavailable, use bounded parallel requests and measure before/after.
- Avoid clearing usable data during refresh; preserve row context and selection only when revisions still match.
- Targets: `src/components/management/org-workspaces.tsx`, `src/components/management/members.tsx`, existing management API wrapper/contract only if required.

### P2 — interaction, accessibility and visual consistency

- Audit all Org/WS/member/group/profile views for loading, empty, no-results, stale/partial, forbidden, conflict, saving, success, destructive confirmation and retry states against `docs/design/screen-state-matrix.md`.
- Test keyboard-only navigation, focus return, table row selection, icon button labels, long names/emails, window resize, and a large Org with many WS/groups/members.
- Use existing Donut/shadcn primitives and semantic tokens. No second management design system.
- Targets: all components listed above plus localized labels; visual reference/acceptance must follow the UI authority and pinned Donut source.

## Decisions to make together before implementation

1. Should an Org Owner enter an Org-scoped “Organization management” page directly, with Platform Admin separate, or keep the shared route but change context/title? Recommended: shared implementation, distinct visible scope, platform-only global actions isolated.
2. Should Org Owner have an explicit owner badge in breadcrumb and row-level “Your access” labels, or only show the Org role in Org detail? Recommended: breadcrumb + detail; avoid duplicating role badges everywhere.
3. Should WS navigation be tabs, nested routes, or split-pane? Recommended: keep the existing compact tabs initially, add route/history semantics and persistent Org/WS breadcrumb; revisit only after interaction QA.
4. Confirm “GR/PR” means workspace Groups and Profiles governance tabs (not browser profile runtime operations).
5. Confirm priority order: P0 scope clarity → P1 hierarchy + access semantics + loading → P2 group/profile detail polish + full visual/a11y pass.
6. For Org/Workspace removal, approve the recommended guided cleanup that resolves dependencies explicitly, or decide whether a separate guarded cascade is required. Current plan recommends guided cleanup first; no silent cascade.

## Acceptance evidence for implementation

- Org Owner screenshot and interaction trace show Org scope and Owner role, never Platform Admin; platform administrator trace clearly shows global scope.
- Same account across two Org/WS scopes cannot confuse which scope each action affects; backend tests continue to enforce actual access.
- Refresh/back/deep-link restores selected Org, WS and relevant view as agreed.
- Account, Org membership, WS membership, role, group grant and effective access can be distinguished from screen and confirmation copy.
- Large Org inventory first paint does not wait for per-WS counts or unbounded group-share requests; measured request count and latency are recorded.
- Partial failures identify exactly which panel failed; successful panels remain usable.
- Full UI state and keyboard/responsive checklist passes with screenshots against current UI authority and Donut reference.
- Org Owner and Platform Admin can discover and complete authorized deletion resolutions; blocker codes always map to a useful action or an explicit wait state.
- Workspace deletion handles all current blocker families; Org deletion guides through child Workspaces and remaining Org members; partial cleanup remains resumable and auditable.
- Removing an Org membership is an explicit, revision-bound operation that preserves the account, refuses the Org owner, and refuses while child Workspace memberships remain.

Org-membership cleanup vertical slice — 2026-10-28:

- Root cause: organization deletion correctly rejects remaining non-owner memberships, but the command/API surface had no remove-membership operation. The UI's “Danh bạ tài khoản” destination therefore could not resolve that blocker. Suspension is reversible and intentionally preserves child membership/grants, so it is not a substitute.
- Contract: add an audited, idempotent `organization.member.remove` command with server-side organization management authorization, expected-revision enforcement, Owner protection, and a child-workspace-membership blocker. Removal affects only the organization-membership record and never deletes the account. Expose the operation in the Org account directory with an explicit reviewed/batch result flow and preserve the existing resumable delete target.
- Acceptance evidence to add: service revision/owner tests; HTTP authorization, idempotency, child-membership blocker and success tests; MCP parity; generated contract checks; desktop typecheck/interaction behavior. Implementing now; no release claim until the full vertical slice verifies.

## Implementation progress — 2026-10-27

- Org and workspace inventory now renders before secondary workspace usage lookups finish. Usage requests remain bounded to four at a time and update each row independently, so a slow count no longer holds the entire page in loading state.
- Organization detail requests are bounded to four organizations per batch; the organization list is retained as the initial visible snapshot while workspace/member details load.
- Desktop typecheck and UI interaction/screenshot QA remain to be rerun after this change.
- Not yet implemented: backend-derived organization capabilities, API-backed revision-bound deletion preflight and guided cleanup, URL/history context, and the remaining account/member/group/profile hierarchy surfaces.
- Desktop typecheck passes after the inventory change. Authenticated screenshot and interaction QA remain pending; do not treat this loading fix as completion of the management plan.
- The management header now uses the backend `AccountView.platformAdmin` result: Platform Admin is labeled only when the server says the actor has that scope; otherwise the route is labeled Organization management. The duplicate, hard-coded “Super Admin” shell title is removed. Permission enforcement remains server-side.
- Management mutation visibility now uses server-derived `manageableOrganizationIds` and `manageableWorkspaceIds`, including explicit capability grants rather than only owner/role guesses.
- Organization and workspace deletion now have authenticated API preflight endpoints with blocker codes/counts and current revisions. The dialog refreshes preflight immediately before delete, blocks stale/blocked submission, and links resolvable workspace blockers to the owning Profiles, Groups, or Members tab. This remains a guided blocker resolution flow, not a full multi-step cleanup wizard.
- Organization/workspace selection, organization tab, search, and page are restored from session storage; workspace subview is remembered per workspace. Organization/workspace IDs and both organization/workspace tabs now round-trip through URL query parameters and browser Back/Forward, preserving unrelated query parameters. The URL is authoritative when present; otherwise the saved session state remains the fallback.
- Workspace member refresh keeps its last usable snapshot visible. Group-share reads now use bounded batches of eight; failed groups retain their prior grants and the error names those groups instead of silently representing them as having no access.
- Group/Profile governance loading also uses bounded batches of eight for group grants, preserves the last known membership snapshot on partial member failures, and names groups whose grant lookup failed. Management member rows now label direct grants explicitly and no longer imply Owner/Admin roles grant access to every group; visibility uses backend-derived workspace capabilities.
- Bulk activate/suspend/remove keeps a partial-result dialog open with only failed members selected, so retry submits the failed subset after refreshing server state instead of repeating successful rows.
- Organization and Workspace create/edit forms now keep validation feedback next to each invalid field, expose errors through `aria-invalid`/`aria-describedby`, clear the field error as it is corrected, and constrain numeric values to their supported integer ranges. Authoritative server record/revision loading and server-side mutation authorization remain unchanged. Biome passes for `org-workspaces.tsx`; root TypeScript currently fails on pre-existing errors in unrelated Cookie Bot, profile, sync, and bookmark components. Screenshot, keyboard, responsive, and authenticated interaction evidence for this form change remains pending.
- Governance assignment picker dialogs now show their parent organization/workspace and selected record count. The direct Groups surface also passes this context instead of rendering an unscoped picker.
- Profile group assignment and member direct group grant dialogs now provide explicit Add/Replace/Remove semantics with per-profile/member current → resulting group previews and changed/unchanged counts. Team inherited access and explicit denies remain outside direct-grant mutations.
- Member roster now shows team membership in its own column, separate from direct group grants, and resolves team names independently so a failed team lookup does not blank the member list. This clarifies membership vs direct grants; team-derived effective rights still need an authoritative server projection before the corresponding checklist item can be closed.
- Direct group-grant rows and mutation previews now include each active allow action (`read`/`execute`) per group. Add/Replace/Remove preview computes action-level before/after state, preserving unselected direct permissions according to the selected mode; deny grants remain outside the mutation and remain effective.
- Profile assignment previews now label group IDs whose names are unavailable instead of incorrectly presenting those existing assignments as “No groups.”
- Ownership transfer no longer renders an empty select over the “no eligible account” state; an actual load failure remains an error with a retry action instead of being mislabeled as an empty organization.
- Current verification: desktop typecheck, desktop Vite build, focused Biome, `cargo check -p buglogin-api --lib`, and `git diff --check` pass. The Vite build still emits existing `use client`, sourcemap, mixed-import, and large-chunk warnings. The initial `http_control` run hit a Windows file lock from the running API; rerunning with an isolated `CARGO_TARGET_DIR` passed all four integration tests. Authenticated screenshot, keyboard, resize, and full hierarchy interaction QA remain pending; do not mark this plan complete.
- Follow-up direct-grant action preview and independent team-membership cell pass desktop typecheck, the full desktop self-test suite, focused Biome and `git diff --check`. Direct-grant preview semantics have regression cases for Add/Replace/Remove.
- Added a focused organization-management route parser/serializer with tests for URL inputs, invalid tabs, clearing the route, workspace subview deep links, and preserving unrelated query parameters. The management component now pushes route/tab changes to browser history and restores them on `popstate`; platform-view initialization no longer erases a restored route. Visual and authenticated navigation QA remain pending.
- Workspace-delete preflight now reports actual resource/access, group/extension-group/access, and team record counts instead of collapsing every non-empty category to `1`; blocker copy displays the count in both locales. Focused API record-count tests pass (4), `cargo check --locked -p buglogin-api --lib` passes, and the `http_control` integration suite passes (4). Authenticated deletion interaction QA remains pending.
- Organization account-directory rows now display account status separately from organization membership status. The previous fallback that mislabeled organization-member status as account status is removed; workspace membership status is also stored separately. If an API scope does not provide account status, the UI states “Not available” rather than inferring it. Added a focused regression self-test; desktop typecheck and screenshot/interaction QA are pending for this change.
- Workspace management now includes a resource inventory tab that deep-links through the existing management URL state. Delete preflight routes profile blockers to Profiles, resource/access blockers to Resources, group blockers to Groups, member/team blockers to Members, and active-operation/sync blockers to a retryable preflight refresh instead of sending all of them to Profiles. Desktop typecheck, self-tests, focused Biome, and Vite production build pass. Authenticated interaction and screenshot QA are pending.
- Account and organization membership status batches now retain only failed account IDs for retry. The retry reopens the existing scoped confirmation for those rows; successful rows are not repeated, and the refreshed list supplies current revisions after the batch reload. Desktop typecheck and focused Biome pass; interactive authenticated QA remains pending.
- Workspace member rows now render as soon as the core member/group/account snapshot arrives; bounded group-share reads finish independently and the direct-grant cells indicate loading. Changing workspace clears the previous scope snapshot, and a request sequence guard prevents late responses from an old scope or refresh overwriting the current view. Desktop typecheck, self-tests and focused Biome pass; authenticated resize/interaction QA remains pending.
- Workspace member access now labels “Direct group grants” and “Team membership” separately and states that team membership does not inherit group access. This matches the current API data model: `Member.teamId` is a grouping relation, and group access is represented by member-targeted grants. Desktop typecheck passes; authenticated visual/accessibility QA remains pending.
- Organization ownership transfer confirmation now states the actual server contract: only Org ownership changes, existing workspace memberships and explicit grants remain as-is, and this action does not grant Platform Admin authority. Pre-confirm copy now describes active-member eligibility and revision recheck accurately.
- Direct group-grant editing is disabled while grant reads are loading or incomplete, so a missing/failed read cannot turn into an empty Replace selection that revokes access. Role-only edits remain available; the dialog explains the unavailable group data and offers the page's reload path. This was found during the authoritative-data audit.
- Org-membership removal is now exposed in the account directory as a reviewed destructive batch action. It is available only for non-owner Org members with no remaining Workspace memberships, reports per-account results, retries only failed rows, and calls the revision-bound `organization.member.remove` command. REST regression verifies child-membership denial, removal after Workspace cleanup, and account preservation. Service/MCP fixtures and API contracts are covered; desktop typecheck and focused Biome pass. Root legacy Next typecheck continues to fail on unrelated missing Donut exports/types; authenticated screenshot and keyboard QA are still open.
- Remaining: account/membership/effective-access distinctions and bulk partial retry audit; broader Group/Profile governance review; visual and accessibility QA; complete all blocker resolution paths and record measured request/latency evidence.
- Follow-up blocker-resolution audit: current Workspace and Organization deletion blocker codes now map through an explicit resolver to the relevant management destination or a retryable preflight refresh. Unknown future blocker codes render localized guidance and a retry action instead of a missing translation or an actionless blocker. A focused self-test covers each current code and the unknown-code fallback. Desktop self-tests, desktop TypeScript typecheck, focused Biome, and `git diff --check` pass; authenticated screenshot/interaction QA remains open.
