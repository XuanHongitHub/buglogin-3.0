"use client";

import { useEffect, useState } from "react";
import styles from "./landing-v2.module.css";
import { PlatformDownloads } from "./platform-downloads";

const platforms = [
  "TikTok Shop",
  "Facebook",
  "Etsy",
  "Shopify",
  "Amazon",
  "eBay",
];

const layers = [
  {
    number: "01",
    title: "Dấu vân tay trình duyệt",
    description:
      "Cấu hình các thuộc tính trình duyệt riêng cho từng hồ sơ để quản lý nhiều môi trường làm việc.",
    tags: ["Canvas", "WebGL", "Ngôn ngữ", "Múi giờ"],
  },
  {
    number: "02",
    title: "Kết nối mạng",
    description:
      "Gắn proxy theo hồ sơ và kiểm tra kết nối trước khi mở trình duyệt.",
    tags: ["HTTP(S)", "SOCKS5", "Kiểm tra IP"],
  },
  {
    number: "03",
    title: "Thông số thiết bị",
    description:
      "Chọn nền tảng và thông số môi trường phù hợp với quy trình sử dụng của bạn.",
    tags: ["Windows", "macOS", "Độ phân giải"],
  },
  {
    number: "04",
    title: "Phiên đăng nhập",
    description:
      "Cookie và dữ liệu trang web được lưu trong vùng dữ liệu riêng của hồ sơ.",
    tags: ["Cookie", "Local Storage", "IndexedDB"],
  },
  {
    number: "05",
    title: "Làm việc nhóm",
    description:
      "Chia sẻ hồ sơ với thành viên theo quyền được cấp trong tổ chức và workspace.",
    tags: ["Vai trò", "Nhóm", "Nhật ký"],
  },
  {
    number: "06",
    title: "Tự động hóa",
    description:
      "Dùng REST API và MCP theo account cùng quyền đã cấp cho khóa truy cập.",
    tags: ["REST API", "MCP", "Playwright"],
  },
];

export function BugLoginLanding() {
  const [scrolled, setScrolled] = useState(false);

  useEffect(() => {
    const update = () => setScrolled(window.scrollY > 24);
    update();
    window.addEventListener("scroll", update, { passive: true });
    return () => window.removeEventListener("scroll", update);
  }, []);

  return (
    <div className={styles.site}>
      <a className={styles.skip} href="#main">
        Bỏ qua đến nội dung
      </a>

      <header
        className={`${styles.header} ${scrolled ? styles.headerScrolled : ""}`}
      >
        <div className={styles.headerInner}>
          <a
            className={styles.brand}
            href="#top"
            aria-label="BugLogin, về đầu trang"
          >
            <img src="/buglogin-logo.png" width="36" height="36" alt="" />
            <span>BugLogin</span>
          </a>
          <nav className={styles.nav} aria-label="Điều hướng chính">
            <a href="#bao-ve">Bảo vệ</a>
            <a href="#nen-tang">Nền tảng</a>
            <a href="#bat-dau">Bắt đầu</a>
            <a href="#hoi-dap">Hỏi đáp</a>
          </nav>
          <a className={styles.headerDownload} href="#download">
            Tải xuống <span aria-hidden="true">↓</span>
          </a>
        </div>
      </header>

      <main id="main">
        <section className={styles.hero} id="top" aria-labelledby="hero-title">
          <div className={styles.heroGrid} aria-hidden="true" />
          <div className={styles.heroInner}>
            <div className={styles.heroCopy}>
              <p className={styles.eyebrow}>
                TRÌNH DUYỆT CHO CÔNG VIỆC NHIỀU TÀI KHOẢN
              </p>
              <h1 id="hero-title">
                Mỗi tài khoản
                <br />
                <span>một hồ sơ riêng.</span>
              </h1>
              <p className={styles.lede}>
                BugLogin giúp tách dữ liệu trình duyệt theo hồ sơ, gắn cấu hình
                mạng riêng và quản lý quyền làm việc trong workspace.
              </p>
              <div className={styles.heroActions} id="download">
                <PlatformDownloads className={styles.heroDownloads} />
                <a className={styles.loginLink} href="/login">
                  Đăng nhập vào BugLogin <span aria-hidden="true">→</span>
                </a>
              </div>
              <p className={styles.heroNote}>
                Có bản tải ổn định cho Windows và macOS khi được phát hành.
              </p>
            </div>

            <div className={styles.heroVisual} aria-hidden="true">
              <div className={styles.visualOrbit} />
              <div className={styles.profilePanel}>
                <div className={styles.panelTop}>
                  <span className={styles.windowDots}>
                    <i />
                    <i />
                    <i />
                  </span>
                  <span>BUGLOGIN PROFILE</span>
                  <span className={styles.panelStatus}>ISOLATED</span>
                </div>
                <div className={styles.fingerprintArt}>
                  <span />
                  <span />
                  <span />
                  <span />
                  <span />
                  <span />
                  <span />
                </div>
                <div className={styles.panelReadout}>
                  <div>
                    <b>01</b>
                    <span>Hồ sơ riêng</span>
                  </div>
                  <div>
                    <b>02</b>
                    <span>Dữ liệu riêng</span>
                  </div>
                  <div>
                    <b>03</b>
                    <span>Quyền rõ ràng</span>
                  </div>
                </div>
              </div>
              <div className={styles.visualCaption}>
                <span className={styles.statusDot} /> Môi trường theo từng hồ sơ
              </div>
            </div>
          </div>

          <div className={styles.platformStrip}>
            <div className={styles.stripInner}>
              <p>DÀNH CHO CÁC NỀN TẢNG BẠN ĐANG VẬN HÀNH</p>
              <ul>
                {platforms.map((platform) => (
                  <li key={platform}>{platform}</li>
                ))}
              </ul>
            </div>
          </div>
        </section>

        <section
          className={`${styles.section} ${styles.paper}`}
          id="bao-ve"
          aria-labelledby="why-title"
        >
          <div className={`${styles.wrap} ${styles.whyGrid}`}>
            <div className={styles.whyCopy}>
              <p className={styles.eyebrowDark}>TÁCH BIỆT CÓ CHỦ ĐÍCH</p>
              <h2 id="why-title">
                Nhiều tài khoản không nên dùng chung một môi trường.
              </h2>
              <p>
                Trình duyệt lưu cookie, phiên đăng nhập và dữ liệu trang web
                trên thiết bị. Khi các tài khoản dùng chung một hồ sơ, dữ liệu
                và phiên làm việc có thể bị lẫn với nhau.
              </p>
              <p>
                BugLogin tạo vùng dữ liệu riêng cho từng profile, giúp bạn tổ
                chức phiên làm việc rõ ràng hơn và mở lại đúng hồ sơ khi cần.
              </p>
              <p className={styles.keyLine}>
                Mỗi hồ sơ có cấu hình và dữ liệu trình duyệt độc lập trên thiết
                bị.
              </p>
            </div>
            <div
              className={styles.compare}
              role="img"
              aria-label="So sánh một môi trường dùng chung với các hồ sơ dữ liệu riêng"
            >
              <div className={styles.compareCard}>
                <span className={styles.compareLabel}>DÙNG CHUNG</span>
                <div className={styles.sharedAccounts}>
                  <i />
                  <i />
                  <i />
                  <i />
                </div>
                <div className={styles.sharedCore}>
                  <b>01</b>
                  <span>Môi trường</span>
                </div>
                <p>Nhiều phiên dùng chung một vùng dữ liệu</p>
              </div>
              <div
                className={`${styles.compareCard} ${styles.compareCardAccent}`}
              >
                <span className={styles.compareLabel}>VỚI BUGLOGIN</span>
                <div className={styles.separateProfiles}>
                  <div>
                    <i />
                    <span>Profile A</span>
                  </div>
                  <div>
                    <i />
                    <span>Profile B</span>
                  </div>
                  <div>
                    <i />
                    <span>Profile C</span>
                  </div>
                  <div>
                    <i />
                    <span>Profile D</span>
                  </div>
                </div>
                <p>Mỗi profile có vùng dữ liệu riêng</p>
              </div>
            </div>
          </div>
        </section>

        <section
          className={`${styles.section} ${styles.white}`}
          aria-labelledby="layers-title"
        >
          <div className={styles.wrap}>
            <header className={styles.sectionHead}>
              <p className={styles.eyebrowDark}>CẤU HÌNH THEO HỒ SƠ</p>
              <h2 id="layers-title">
                Mỗi hồ sơ là một môi trường làm việc riêng.
              </h2>
              <p>
                Quản lý các lớp cấu hình và dữ liệu ngay trong ứng dụng
                BugLogin.
              </p>
            </header>
            <ol className={styles.layerList}>
              {layers.map((layer) => (
                <li className={styles.layer} key={layer.number}>
                  <span className={styles.layerNumber}>{layer.number}</span>
                  <div className={styles.layerCopy}>
                    <h3>{layer.title}</h3>
                    <p>{layer.description}</p>
                  </div>
                  <ul className={styles.tags}>
                    {layer.tags.map((tag) => (
                      <li key={tag}>{tag}</li>
                    ))}
                  </ul>
                </li>
              ))}
            </ol>
          </div>
        </section>

        <section
          className={`${styles.section} ${styles.paper}`}
          id="nen-tang"
          aria-labelledby="platform-title"
        >
          <div className={styles.wrap}>
            <header className={styles.sectionHead}>
              <p className={styles.eyebrowDark}>LÀM VIỆC TRÊN NHIỀU NỀN TẢNG</p>
              <h2 id="platform-title">Dành cho các sàn bạn đang bán.</h2>
              <p>
                Sắp xếp profile theo shop, tài khoản hoặc nhóm vận hành của bạn.
              </p>
            </header>
            <ul className={styles.marketGrid}>
              {platforms.map((platform, index) => (
                <li key={platform}>
                  <span className={styles.marketNumber}>0{index + 1}</span>
                  <h3>{platform}</h3>
                  <p>
                    Quản lý phiên làm việc trong profile riêng, dễ nhận biết và
                    chuyển đổi.
                  </p>
                </li>
              ))}
            </ul>
          </div>
        </section>

        <section
          className={`${styles.section} ${styles.white}`}
          id="bat-dau"
          aria-labelledby="steps-title"
        >
          <div className={styles.wrap}>
            <header className={styles.sectionHead}>
              <p className={styles.eyebrowDark}>BẮT ĐẦU</p>
              <h2 id="steps-title">Bắt đầu trong ba bước.</h2>
            </header>
            <ol className={styles.stepList}>
              <li>
                <b>01</b>
                <h3>Tải và cài đặt</h3>
                <p>Chọn bản BugLogin phù hợp với Windows hoặc macOS.</p>
              </li>
              <li>
                <b>02</b>
                <h3>Tạo hồ sơ</h3>
                <p>
                  Đặt cấu hình profile và gắn proxy nếu quy trình của bạn cần.
                </p>
              </li>
              <li>
                <b>03</b>
                <h3>Mở và làm việc</h3>
                <p>Khởi chạy profile để dùng vùng dữ liệu trình duyệt riêng.</p>
              </li>
            </ol>
          </div>
        </section>

        <section
          className={`${styles.section} ${styles.paper}`}
          id="hoi-dap"
          aria-labelledby="faq-title"
        >
          <div className={`${styles.wrap} ${styles.faq}`}>
            <p className={styles.eyebrowDark}>HỎI ĐÁP</p>
            <h2 id="faq-title">Câu hỏi thường gặp.</h2>
            <details>
              <summary>
                Trình duyệt antidetect khác gì chế độ ẩn danh?
                <span aria-hidden="true">+</span>
              </summary>
              <p>
                Chế độ ẩn danh chủ yếu hạn chế dữ liệu được lưu sau khi đóng cửa
                sổ. BugLogin quản lý profile riêng với vùng dữ liệu và cấu hình
                trình duyệt riêng trên thiết bị.
              </p>
            </details>
            <details>
              <summary>
                Có cần một proxy riêng cho mỗi hồ sơ không?
                <span aria-hidden="true">+</span>
              </summary>
              <p>
                Tùy quy trình của bạn. BugLogin cho phép gắn và kiểm tra proxy
                theo profile; hãy chọn cấu hình mạng phù hợp với cách bạn vận
                hành từng tài khoản.
              </p>
            </details>
            <details>
              <summary>
                Nhân viên có thể dùng profile được chia sẻ không?
                <span aria-hidden="true">+</span>
              </summary>
              <p>
                Có, khi thành viên được cấp quyền truy cập profile trong
                workspace. Thao tác được giới hạn theo role và quyền của
                account.
              </p>
            </details>
            <details>
              <summary>
                BugLogin có bản cho Windows và macOS không?
                <span aria-hidden="true">+</span>
              </summary>
              <p>
                Trang này đọc stable release feed và chỉ hiển thị link cho nền
                tảng có artifact được phát hành. Phiên bản macOS hiện nhắm tới
                Apple Silicon.
              </p>
            </details>
            <details>
              <summary>
                Profile có tự đồng bộ sang máy khác không?
                <span aria-hidden="true">+</span>
              </summary>
              <p>
                Việc sử dụng profile trên thiết bị khác phụ thuộc quyền truy
                cập, cấu hình đồng bộ, khóa mã hóa và khả năng tương thích dữ
                liệu giữa hệ điều hành/runtime.
              </p>
            </details>
          </div>
        </section>

        <section className={styles.final} aria-labelledby="final-title">
          <div className={styles.finalInner}>
            <p className={styles.eyebrow}>BUGLOGIN DESKTOP</p>
            <h2 id="final-title">Sẵn sàng sắp xếp profile riêng?</h2>
            <p>Chọn bản phát hành ổn định hiện có cho thiết bị của bạn.</p>
            <PlatformDownloads className={styles.finalDownloads} />
          </div>
        </section>
      </main>

      <footer className={styles.footer}>
        <div className={styles.footerTop}>
          <div className={styles.footerBrand}>
            <a className={styles.brand} href="#top">
              <img src="/buglogin-logo.png" width="32" height="32" alt="" />
              <span>BugLogin</span>
            </a>
            <p>
              Quản lý các profile trình duyệt trong một ứng dụng dành cho
              Windows và macOS.
            </p>
          </div>
          <nav aria-label="Liên kết cuối trang">
            <a href="/security">Bảo mật</a>
            <a href="/docs">Tài liệu</a>
            <a href="/download">Tải về</a>
            <a href="mailto:support@buglogin.com">Liên hệ</a>
          </nav>
        </div>
        <div className={styles.footerBottom}>
          <span>© {new Date().getFullYear()} BugLogin</span>
          <span>Windows · macOS</span>
        </div>
      </footer>
    </div>
  );
}
