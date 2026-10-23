import { useCallback, useState } from "react";
import { Button, Panel, Status } from "../../../design-system";
import type { LoadState, PublicSummary, WebApiClient } from "../api";
import { useLoadState, WebShell } from "../shells";
import { BugLoginLanding } from "./landing.tsx";
import { PlatformDownloads } from "./platform-downloads.tsx";

const neverEmpty = () => false;
const publicNav = [
  { href: "/", label: "Trang chủ" },
  { href: "/features", label: "Tính năng" },
  { href: "/pricing", label: "Bảng giá" },
  { href: "/security", label: "Bảo mật E2EE" },
  { href: "/docs", label: "Tài liệu" },
  { href: "/download", label: "Tải về" },
  { href: "/status", label: "Trạng thái" },
];

const PIXEL_BUG_ART = `
        .·\\::++::/·.        
       /  ·:×◎◎×:·  \\       
      / .·  |  |  ·. \\      
    _/_/   .·×▣▣×·.   \\_\\_  
   [◎]    /::×□□×::\\    [◎] 
   /  \\  |::×▣▣  ▣▣×::|  /  \\ 
 /\\__/  \\|::×□□  □□×::|/  \\__/\\
/  \\     /\\::×▣▣▣▣×::/\\     /  \\
|   \\___/  \\::×++×::/  \\___/   |
|    \\   .·--\\____/--·.   /    |
 \\ /\\ \\ / .·×□□□□□□×·. \\ / /\\ / 
  V  \\ | :×▣▣▣▣▣▣▣▣▣▣×: | /  V  
      \\| :×□□[E2EE]□□×: |/      
       | :×▣▣[VAULT]▣▣×:|       
       \\  \\:×▣▣▣▣▣▣×:/  /       
        \\  \\.·×□□×·./  /        
         \\___/    \\___/         
          |          |          
         /            \\         
`;

function PixelBugFlank({ side }: { side: "left" | "right" }) {
  return (
    <pre
      aria-hidden="true"
      className={`buglogin-pixel-bug-flank buglogin-pixel-bug-flank--${side}`}
    >
      {PIXEL_BUG_ART.trim()}
    </pre>
  );
}

function HeroBugShowcase() {
  return (
    <div
      className="buglogin-hero-bug-showcase"
      role="group"
      aria-label="BugLogin Antidetect Engine"
    >
      <div className="buglogin-hero-bug-glow-orb">
        <img
          src="/buglogin-logo.png"
          alt="BugLogin Crimson Beetle"
          className="buglogin-hero-bug-img"
        />
      </div>

      <div className="buglogin-hero-telemetry-grid">
        <div className="buglogin-telemetry-chip">
          <span className="buglogin-telemetry-dot" />
          <span>Canvas: Perturbed</span>
        </div>
        <div className="buglogin-telemetry-chip">
          <span className="buglogin-telemetry-dot" />
          <span>WebGL: Metal / RTX</span>
        </div>
        <div className="buglogin-telemetry-chip">
          <span className="buglogin-telemetry-dot" />
          <span>WebRTC: 0-Leak UDP</span>
        </div>
        <div className="buglogin-telemetry-chip">
          <span className="buglogin-telemetry-dot buglogin-telemetry-dot--crimson" />
          <span>Proxy: SOCKS5 Active</span>
        </div>
      </div>
    </div>
  );
}

export function PublicPage({ api, path }: { api: WebApiClient; path: string }) {
  const load = useCallback(
    (signal: AbortSignal) => api.reads.publicSummary(signal),
    [api],
  );
  const needsSummary = path.startsWith("/status");
  const { state, retry } = useLoadState(
    load,
    neverEmpty,
    "Dữ liệu nền tảng tạm thời không khả dụng.",
    needsSummary,
  );
  const summary = state.status === "ready" ? state.data : undefined;

  if (path === "/" || path === "") return <BugLoginLanding />;

  let page = <HomePage summary={summary} state={state} retry={retry} />;
  if (path.startsWith("/download")) {
    page = <DownloadPage />;
  } else if (path.startsWith("/features")) {
    page = <FeaturesPage />;
  } else if (path.startsWith("/pricing")) {
    page = <PricingPage />;
  } else if (path.startsWith("/security")) {
    page = <SecurityPage />;
  } else if (path.startsWith("/docs")) {
    page = <DocumentationPage />;
  } else if (path.startsWith("/status")) {
    page = <StatusPage summary={summary} state={state} retry={retry} />;
  }

  return (
    <WebShell activePath={path} mode="public" nav={publicNav} title="BugLogin">
      {page}
    </WebShell>
  );
}

function HomePage({
  summary,
  state,
  retry,
}: {
  summary?: PublicSummary;
  state: LoadState<PublicSummary>;
  retry: () => void;
}) {
  const [activeTab, setActiveTab] = useState<"playwright" | "proxy">(
    "playwright",
  );
  const [copied, setCopied] = useState(false);

  const playwrightCode = `import { chromium } from "playwright";

// Kết nối trực tiếp vào Profile BugLogin qua CDP WebSocket
const browser = await chromium.connectOverCDP(
  "wss://api.buglogin.com/v1/cdp?profile=bl_prf_live_01"
);
const page = await browser.contexts()[0].newPage();
await page.goto("https://target-service.com");`;

  const proxyCode = `$ buglogin-mesh --status --verify-p0f
[● POOL STATUS]        : 40M+ Residential IPs (195 Countries)
[● TCP OS FINGERPRINT] : MATCHED (Windows 11 / macOS MTU 1500)
[● EXIT NODE]          : 104.28.42.19 (SG Residential) - Latency: 22ms`;

  const handleCopy = () => {
    const textToCopy = activeTab === "playwright" ? playwrightCode : proxyCode;
    navigator.clipboard.writeText(textToCopy);
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
  };

  return (
    <>
      {/* HERO SECTION WITH PIXEL FLANKS & HERO BUG */}
      <section className="buglogin-hero-stage" aria-labelledby="public-title">
        <PixelBugFlank side="left" />
        <PixelBugFlank side="right" />

        <div className="buglogin-hero-grid">
          <div>
            <p className="buglogin-public-eyebrow">
              BUGLOGIN • NỀN TẢNG TRÌNH DUYỆT DOANH NGHIỆP
            </p>
            <h1 id="public-title" className="buglogin-hero-headline">
              Hạ tầng trình duyệt Antidetect bảo mật cho doanh nghiệp.
            </h1>
            <p className="buglogin-hero-lead">
              Vận hành hàng nghìn hồ sơ độc lập, cô lập tuyệt đối phần cứng và
              mạng. Mã hóa dữ liệu đầu cuối E2EE, phân quyền chống rò rỉ thông
              tin tập trung.
            </p>

            <div className="buglogin-public-actions">
              {summary?.release ? (
                <a
                  className="buglogin-btn-primary-crimson"
                  href={summary.release.downloadUrl}
                >
                  <svg
                    aria-hidden="true"
                    viewBox="0 0 24 24"
                    width="16"
                    height="16"
                    stroke="currentColor"
                    strokeWidth="2"
                    fill="none"
                    style={{ display: "inline-block" }}
                  >
                    <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" />
                    <polyline points="7 10 12 15 17 10" />
                    <line x1="12" y1="15" x2="12" y2="3" />
                  </svg>
                  <span>Tải Desktop v{summary.release.version}</span>
                </a>
              ) : (
                <a className="buglogin-btn-primary-crimson" href="/download">
                  <svg
                    aria-hidden="true"
                    viewBox="0 0 24 24"
                    width="16"
                    height="16"
                    stroke="currentColor"
                    strokeWidth="2"
                    fill="none"
                    style={{ display: "inline-block" }}
                  >
                    <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" />
                    <polyline points="7 10 12 15 17 10" />
                    <line x1="12" y1="15" x2="12" y2="3" />
                  </svg>
                  <span>Tải Desktop Client</span>
                </a>
              )}
              <a className="buglogin-btn-secondary-cyber" href="/workspaces">
                <svg
                  aria-hidden="true"
                  viewBox="0 0 24 24"
                  width="16"
                  height="16"
                  stroke="currentColor"
                  strokeWidth="2"
                  fill="none"
                  style={{ display: "inline-block" }}
                >
                  <rect x="3" y="3" width="18" height="18" rx="2" />
                  <path d="M9 3v18" />
                  <path d="M14 9h5" />
                  <path d="M14 15h5" />
                </svg>
                <span>Mở Web Dashboard</span>
              </a>
              <a className="buglogin-btn-secondary-cyber" href="/security">
                <svg
                  aria-hidden="true"
                  viewBox="0 0 24 24"
                  width="16"
                  height="16"
                  stroke="currentColor"
                  strokeWidth="2"
                  fill="none"
                  style={{ display: "inline-block" }}
                >
                  <rect x="3" y="11" width="18" height="11" rx="2" ry="2" />
                  <path d="M7 11V7a5 5 0 0 1 10 0v4" />
                </svg>
                <span>Khám phá Bảo mật</span>
              </a>
            </div>

            <div
              className="buglogin-trust-ticker"
              role="group"
              aria-label="Cam kết chất lượng nền tảng"
            >
              <div className="buglogin-trust-item">
                <span style={{ color: "#e11d48" }}>✓</span>
                <span>
                  <strong>Zero-Telemetry:</strong> Ứng dụng không theo dõi
                </span>
              </div>
              <div className="buglogin-trust-item">
                <span style={{ color: "#e11d48" }}>✓</span>
                <span>
                  <strong>Lưu trữ an toàn:</strong> Cục bộ hoặc Cloud E2EE
                </span>
              </div>
              <div className="buglogin-trust-item">
                <span style={{ color: "#e11d48" }}>✓</span>
                <span>
                  <strong>Chống rò rỉ:</strong> WebRTC UDP Bound
                </span>
              </div>
              <div className="buglogin-trust-item">
                <span style={{ color: "#e11d48" }}>✓</span>
                <span>
                  <strong>AI Ready:</strong> Playwright & MCP
                </span>
              </div>
            </div>
          </div>

          <HeroBugShowcase />
        </div>
      </section>

      {/* BENTO HIGHLIGHTS SECTION */}
      <section className="buglogin-bento-section" aria-labelledby="bento-title">
        <div className="buglogin-bento-header">
          <p className="buglogin-public-eyebrow">CÔNG NGHỆ CỐT LÕI</p>
          <h2 id="bento-title">
            Đơn giản, trực quan và bảo vệ toàn diện tài sản số
          </h2>
        </div>

        <div className="buglogin-bento-grid">
          <article className="buglogin-bento-card">
            <div className="buglogin-bento-icon">
              <svg
                aria-hidden="true"
                viewBox="0 0 24 24"
                width="22"
                height="22"
                stroke="currentColor"
                strokeWidth="2"
                fill="none"
              >
                <path d="M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z" />
              </svg>
            </div>
            <h3>Cô Lập Vân Tay Đa Tầng</h3>
            <p>
              Mỗi Profile chạy trong môi trường phần cứng giả lập riêng: Canvas
              2D, WebGL, AudioContext và WebRTC IP Shield hoàn toàn tự nhiên.
            </p>
          </article>

          <article className="buglogin-bento-card">
            <div className="buglogin-bento-icon">
              <svg
                aria-hidden="true"
                viewBox="0 0 24 24"
                width="22"
                height="22"
                stroke="currentColor"
                strokeWidth="2"
                fill="none"
              >
                <circle cx="12" cy="12" r="10" />
                <line x1="2" y1="12" x2="22" y2="12" />
                <path d="M12 2a15.3 15.3 0 0 1 4 10 15.3 15.3 0 0 1-4 10 15.3 15.3 0 0 1-4-10 15.3 15.3 0 0 1 4-10z" />
              </svg>
            </div>
            <h3>Mạng Proxy Đa Giao Thức</h3>
            <p>
              Tương thích HTTP, HTTPS và SOCKS5. Kiểm tra độ trễ ping và xoay
              vòng IP tự động, giúp duy trì kết nối ổn định liên tục.
            </p>
          </article>

          <article className="buglogin-bento-card">
            <div className="buglogin-bento-icon">
              <svg
                aria-hidden="true"
                viewBox="0 0 24 24"
                width="22"
                height="22"
                stroke="currentColor"
                strokeWidth="2"
                fill="none"
              >
                <rect x="3" y="11" width="18" height="11" rx="2" ry="2" />
                <path d="M7 11V7a5 5 0 0 1 10 0v4" />
              </svg>
            </div>
            <h3>Mã Hóa E2EE Zero-Knowledge</h3>
            <p>
              Cookies và dữ liệu phiên làm việc được mã hóa tại máy client trước
              khi đồng bộ lên S3 Vault. Máy chủ không bao giờ đọc được dữ liệu
              thật.
            </p>
          </article>

          <article className="buglogin-bento-card">
            <div className="buglogin-bento-icon">
              <svg
                aria-hidden="true"
                viewBox="0 0 24 24"
                width="22"
                height="22"
                stroke="currentColor"
                strokeWidth="2"
                fill="none"
              >
                <path d="M17 21v-2a4 4 0 0 0-4-4H5a4 4 0 0 0-4 4v2" />
                <circle cx="9" cy="7" r="4" />
                <path d="M23 21v-2a4 4 0 0 0-3-3.87" />
                <path d="M16 3.13a4 4 0 0 1 0 7.75" />
              </svg>
            </div>
            <h3>Phân Quyền Chống Mất Cắp (DLP)</h3>
            <p>
              Bàn giao Profile cho nhân viên vận hành nhưng ẩn hoàn toàn Raw
              Cookie và Pass Proxy, ngăn chặn triệt để nguy cơ thất thoát dữ
              liệu.
            </p>
          </article>
        </div>
      </section>

      {/* DEVELOPER TERMINAL PREVIEW */}
      <section
        className="buglogin-terminal-section"
        aria-labelledby="dev-title"
      >
        <div className="buglogin-bento-header">
          <p className="buglogin-public-eyebrow">TỰ ĐỘNG HÓA NHANH CHÓNG</p>
          <h2 id="dev-title">Sẵn sàng điều khiển bằng Code hoặc AI Agents</h2>
        </div>

        <div className="buglogin-terminal-card">
          <div className="buglogin-terminal-header">
            <div className="buglogin-terminal-tabs">
              <button
                type="button"
                className={`buglogin-terminal-tab ${activeTab === "playwright" ? "buglogin-terminal-tab--active" : ""}`}
                onClick={() => setActiveTab("playwright")}
              >
                Playwright CDP
              </button>
              <button
                type="button"
                className={`buglogin-terminal-tab ${activeTab === "proxy" ? "buglogin-terminal-tab--active" : ""}`}
                onClick={() => setActiveTab("proxy")}
              >
                Kiểm tra Proxy Mesh
              </button>
            </div>
            <button
              type="button"
              className="buglogin-terminal-tab"
              onClick={handleCopy}
              style={{ color: copied ? "#10b981" : undefined }}
            >
              {copied ? "✓ Đã sao chép" : "Sao chép"}
            </button>
          </div>
          <div className="buglogin-terminal-body">
            <pre style={{ margin: 0, whiteSpace: "pre-wrap" }}>
              {activeTab === "playwright" ? playwrightCode : proxyCode}
            </pre>
          </div>
        </div>
      </section>

      {/* QUICK STATUS */}
      <section
        className="buglogin-public-status"
        aria-labelledby="service-title"
      >
        <div>
          <p className="buglogin-public-eyebrow">HỆ THỐNG TRỰC TUYẾN</p>
          <h2 id="service-title">Trạng thái Nền tảng BugLogin</h2>
        </div>
        <LiveStatus summary={summary} state={state} retry={retry} />
      </section>
    </>
  );
}

function PricingPage() {
  return (
    <section className="buglogin-public-route" aria-labelledby="pricing-title">
      <p className="buglogin-public-eyebrow">BẢNG GIÁ MINH BẠCH</p>
      <h1 id="pricing-title">
        Lựa chọn gói dịch vụ phù hợp với quy mô doanh nghiệp
      </h1>
      <p className="buglogin-public-lede">
        Không phát sinh chi phí ẩn. Hạn mức rõ ràng và hỗ trợ mở rộng tức thì
        khi đội ngũ của bạn tăng trưởng.
      </p>

      <div
        style={{
          display: "grid",
          gridTemplateColumns: "repeat(auto-fit, minmax(260px, 1fr))",
          gap: "20px",
          marginTop: "24px",
        }}
      >
        <Panel
          title="Starter (Cá Nhân Thử Nghiệm)"
          description="Dành cho chuyên viên kiểm thử hoặc cá nhân cần bảo vệ danh tính cơ bản."
          actions={
            <a className="buglogin-btn-secondary-cyber" href="/download">
              Bắt đầu miễn phí
            </a>
          }
        >
          <div
            style={{ fontSize: "24px", fontWeight: 700, margin: "8px 0 16px" }}
          >
            0 đ{" "}
            <span
              style={{
                fontSize: "13px",
                fontWeight: 400,
                color: "var(--muted-foreground)",
              }}
            >
              / vĩnh viễn
            </span>
          </div>
          <ul
            style={{
              paddingLeft: "18px",
              fontSize: "13px",
              lineHeight: "1.8",
              color: "var(--muted-foreground)",
            }}
          >
            <li>Tối đa 10 Profiles trình duyệt cục bộ</li>
            <li>1 Không gian làm việc (Workspace) cá nhân</li>
            <li>Lõi BugLogin Antidetect Engine tiêu chuẩn</li>
            <li>Quản lý Proxy SOCKS5/HTTP thủ công</li>
            <li>Hỗ trợ cộng đồng qua Docs</li>
          </ul>
        </Panel>

        <Panel
          title="Team Pro (Đội Nhóm Vận Hành)"
          description="Dành cho đội ngũ Marketing, E-commerce, Media Agency cần phối hợp làm việc nhóm."
          actions={
            <a className="buglogin-btn-primary-crimson" href="/login">
              Đăng ký Team Pro
            </a>
          }
        >
          <div
            style={{
              fontSize: "24px",
              fontWeight: 700,
              margin: "8px 0 16px",
              color: "#e11d48",
            }}
          >
            499.000 đ{" "}
            <span
              style={{
                fontSize: "13px",
                fontWeight: 400,
                color: "var(--muted-foreground)",
              }}
            >
              / tháng
            </span>
          </div>
          <ul
            style={{
              paddingLeft: "18px",
              fontSize: "13px",
              lineHeight: "1.8",
              color: "var(--muted-foreground)",
            }}
          >
            <li>
              <strong>300 - 1.000 Profiles</strong> đồng bộ đám mây E2EE
            </li>
            <li>Tối đa 10 thành viên cùng làm việc</li>
            <li>Phân quyền theo Nhóm Profile (Group Scopes)</li>
            <li>Chính sách DLP: Ẩn raw cookie, che mật khẩu proxy</li>
            <li>Xoay vòng Proxy tự động & Kiểm tra IP theo quốc gia</li>
            <li>Mở profile khi mất mạng (Offline Entitled)</li>
          </ul>
        </Panel>

        <Panel
          title="Enterprise Scale (Doanh Nghiệp Lớn)"
          description="Dành cho doanh nghiệp quy mô lớn cần bảo mật tuyệt đối, SLA cao và tùy chỉnh sâu."
          actions={
            <a
              className="buglogin-btn-secondary-cyber"
              href="mailto:support@buglogin.com"
            >
              Liên hệ Doanh Nghiệp
            </a>
          }
        >
          <div
            style={{ fontSize: "24px", fontWeight: 700, margin: "8px 0 16px" }}
          >
            Tùy chỉnh{" "}
            <span
              style={{
                fontSize: "13px",
                fontWeight: 400,
                color: "var(--muted-foreground)",
              }}
            >
              / theo nhu cầu
            </span>
          </div>
          <ul
            style={{
              paddingLeft: "18px",
              fontSize: "13px",
              lineHeight: "1.8",
              color: "var(--muted-foreground)",
            }}
          >
            <li>
              <strong>Không giới hạn Profiles</strong> & Workspaces
            </li>
            <li>Super Admin Dashboard toàn quyền tổ chức</li>
            <li>Hệ thống phân quyền thứ bậc 5 cấp (Hierarchical RBAC)</li>
            <li>Nhật ký kiểm toán bảo mật đầy đủ (Security Audit Logs)</li>
            <li>REST API & Model Context Protocol (MCP) tự động hóa</li>
            <li>Hỗ trợ kỹ thuật ưu tiên 24/7</li>
          </ul>
        </Panel>
      </div>
    </section>
  );
}

function FeaturesPage() {
  return (
    <section className="buglogin-public-route" aria-labelledby="features-title">
      <p className="buglogin-public-eyebrow">TÍNH NĂNG TOÀN DIỆN</p>
      <h1 id="features-title">
        Công nghệ bảo vệ danh tính số và hạ tầng quản trị chuẩn mực
      </h1>
      <p className="buglogin-public-lede">
        Được kiến trúc bằng ngôn ngữ Rust và trình duyệt Chromium tùy biến,
        BugLogin loại bỏ hoàn toàn các nguy cơ rò rỉ dấu vân tay và dữ liệu nội
        bộ.
      </p>

      <div className="buglogin-public-policy-list">
        <article>
          <h2>1. Cô Lập Dấu Vân Tay Thực (Hardware Fingerprint Isolation)</h2>
          <p>
            BugLogin can thiệp trực tiếp từ tầng lõi của engine trình duyệt, tạo
            ra dấu vân tay phần cứng hoàn toàn tự nhiên và nhất quán: Canvas
            2D/3D, WebGL Vendor/Renderer, AudioContext Buffer, Danh sách Font
            chữ thực, Màn hình & Độ phân giải màn hình.
          </p>
        </article>

        <article>
          <h2>2. Hệ Thống Phân Quyền Thứ Bậc (Hierarchical RBAC)</h2>
          <p>
            Mô hình phân cấp quản trị nghiêm ngặt: Chủ sở hữu (Owner) &gt; Quản
            trị viên (Admin) &gt; Trưởng nhóm (Manager) &gt; Thành viên
            (Member). Thành viên cấp dưới không thể sửa đổi, nâng quyền hoặc đặt
            lại mật khẩu cho thành viên cấp trên.
          </p>
        </article>

        <article>
          <h2>3. Chính Sách Chống Rò Rỉ Dữ Liệu (DLP Guardrails)</h2>
          <p>
            Chặn nhân viên xuất chuỗi cookie thô (Masked Raw Cookie), tự động ẩn
            tài khoản/mật khẩu Proxy nhưng vẫn cho phép sử dụng mạng, ngăn chặn
            việc sao chép hay đánh cắp tài sản tài khoản của doanh nghiệp.
          </p>
        </article>

        <article>
          <h2>4. Đồng Bộ Đám Mây Mã Hóa Đầu Cuối (E2EE Zero-Knowledge)</h2>
          <p>
            Mọi mật khẩu, cookie phiên làm việc và lịch sử duyệt web đều được mã
            hóa bằng khóa bí mật cục bộ do người dùng nắm giữ. Máy chủ BugLogin
            không giữ chìa khóa giải mã.
          </p>
        </article>

        <article>
          <h2>
            5. Tự Động Hóa Qua REST API & Chuẩn MCP (Model Context Protocol)
          </h2>
          <p>
            Cung cấp Local HTTP Server bảo mật bằng token để lập trình viên và
            các AI Agent điều khiển mở, đóng, cấu hình profile qua Playwright,
            Puppeteer, Selenium.
          </p>
        </article>
      </div>
    </section>
  );
}

function DownloadPage() {
  return <PlatformDownloads standalone />;
}

function SecurityPage() {
  return (
    <section className="buglogin-public-route" aria-labelledby="security-title">
      <p className="buglogin-public-eyebrow">RANH GIỚI BẢO MẬT & ĐỘ TIN CẬY</p>
      <h1 id="security-title">Kiến Trúc An Toàn & Bảo Mật Dữ Liệu</h1>
      <p className="buglogin-public-lede">
        Tại BugLogin, bảo mật không phải là tính năng bổ sung — đó là nền tảng
        cốt lõi trong mọi dòng mã Rust và giao diện người dùng.
      </p>

      <div className="buglogin-public-policy-list">
        <article>
          <h2>Quyền kiểm soát thuộc về máy cục bộ (Desktop Authority)</h2>
          <p>
            Mã nguồn Rust sở hữu quyền điều khiển hệ thống tệp tin, tiến trình
            trình duyệt Chromium, bộ lưu trữ SQLite cục bộ và các chứng chỉ
            mạng. Không có bất kỳ tiến trình nào chạy ngầm ngoài tầm kiểm soát
            của người dùng.
          </p>
        </article>
        <article>
          <h2>Mã hóa mật khẩu Argon2id & E2EE Zero-Knowledge</h2>
          <p>
            Mật khẩu thành viên và mật khẩu tài khoản được băm một chiều bằng
            chuẩn Argon2id mới nhất. Khóa mã hóa dữ liệu profile không bao giờ
            lưu trữ trên máy chủ đám mây, đảm bảo quyền riêng tư tuyệt đối.
          </p>
        </article>
        <article>
          <h2>Nhật ký kiểm toán bảo mật bất biến (Audit Logs)</h2>
          <p>
            Mọi thao tác quan trọng: đăng nhập thiết bị mới, chuyển đổi vai trò,
            cấp quyền truy cập profile, đổi mật khẩu đều được ghi lại với dấu
            thời gian chính xác, địa chỉ IP và định danh tài khoản thực hiện.
          </p>
        </article>
      </div>

      <p className="buglogin-public-note">
        [!] Cam kết không thu thập dữ liệu duyệt web cá nhân, không can thiệp
        nội dung web traffic và bảo vệ quyền sở hữu dữ liệu của khách hàng.
      </p>
    </section>
  );
}

function DocumentationPage() {
  return (
    <section className="buglogin-public-route" aria-labelledby="docs-title">
      <p className="buglogin-public-eyebrow">HƯỚNG DẪN KỸ THUẬT</p>
      <h1 id="docs-title">Tài Liệu Phát Triển & Vận Hành</h1>
      <p className="buglogin-public-lede">
        Tài liệu chi tiết giúp lập trình viên và quản trị viên dễ dàng tích hợp
        BugLogin vào quy trình làm việc tự động.
      </p>

      <div className="buglogin-public-policy-list">
        <article>
          <h2>1. Cài đặt và Khởi tạo Workspace</h2>
          <p>
            Hướng dẫn tạo tổ chức, thiết lập không gian làm việc đầu tiên, thêm
            thành viên và chỉ định hạn mức profile.
          </p>
          <a href="/download">Tải bản Desktop để bắt đầu</a>
        </article>
        <article>
          <h2>2. Tự Động Hóa Với REST API</h2>
          <p>
            BugLogin Desktop tích hợp sẵn HTTP server bảo mật (cổng mặc định
            10108). Cung cấp các endpoint: /v1/profiles, /v1/profiles/:id/start,
            /v1/profiles/:id/stop, /v1/proxies.
          </p>
          <a href="/features">Xem danh mục API</a>
        </article>
        <article>
          <h2>3. Tích Hợp Chuẩn MCP (Model Context Protocol) Cho AI Agents</h2>
          <p>
            Cho phép Claude, OpenAI, Antigravity AI kết nối an toàn để điều phối
            tự động duyệt web theo thời gian thực mà không sợ bị chặn IP hay
            khóa tài khoản.
          </p>
          <a href="/security">Đọc về kiến trúc MCP</a>
        </article>
      </div>
    </section>
  );
}

function StatusPage({
  summary,
  state,
  retry,
}: {
  summary?: PublicSummary;
  state: LoadState<PublicSummary>;
  retry: () => void;
}) {
  return (
    <section className="buglogin-public-route" aria-labelledby="status-title">
      <p className="buglogin-public-eyebrow">GIÁM SÁT HỆ THỐNG TRỰC TIẾP</p>
      <h1 id="status-title">Trạng Thái Dịch Vụ Nền Tảng</h1>
      <p className="buglogin-public-lede">
        Thông tin về tình trạng máy chủ đồng bộ, cổng kiểm tra proxy và hệ thống
        cấp phép được cập nhật liên tục.
      </p>
      <LiveStatus summary={summary} state={state} retry={retry} />
    </section>
  );
}

function LiveStatus({
  summary,
  state,
  retry,
}: {
  summary?: PublicSummary;
  state: LoadState<PublicSummary>;
  retry: () => void;
}) {
  if (summary) {
    return (
      <Panel
        className="buglogin-public-status-panel"
        title="Trạng thái hệ thống thời gian thực"
        description={
          summary.status.detail ||
          "Toàn bộ máy chủ API, dịch vụ đám mây E2EE và nút proxy đang vận hành bình thường."
        }
        actions={
          <Status
            label={
              summary.status.label || "Hoạt động bình thường (Operational)"
            }
            tone={
              summary.status.label.toLowerCase().includes("operational") ||
              summary.status.label.toLowerCase().includes("hoạt động")
                ? "success"
                : "warning"
            }
          />
        }
      />
    );
  }
  if (state.status === "loading") {
    return (
      <Panel
        className="buglogin-public-status-panel"
        aria-live="polite"
        title="Đang kiểm tra kết nối dịch vụ"
        description="Đang gửi yêu cầu kiểm tra tình trạng đến máy chủ BugLogin..."
      />
    );
  }
  if (state.status === "error") {
    return (
      <Panel
        className="buglogin-public-status-panel"
        title="Không thể tải trạng thái trực tiếp"
        description={state.message}
        actions={<Button onClick={retry}>Thử lại</Button>}
      />
    );
  }
  return (
    <Panel
      className="buglogin-public-status-panel"
      title="Thông tin nền tảng khả dụng"
      description="Các dịch vụ đang duy trì thời gian phản hồi dưới 45ms. Không có sự cố nào được ghi nhận."
      actions={
        <a className="buglogin-btn-secondary-cyber" href="/download">
          Tải ứng dụng
        </a>
      }
    />
  );
}
