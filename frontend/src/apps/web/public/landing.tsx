"use client";

import { useEffect, useState } from "react";
import styles from "./landing.module.css";
import { PlatformDownloads } from "./platform-downloads";

export function BugLoginLanding() {
  const [scrolled, setScrolled] = useState(false);

  useEffect(() => {
    const update = () => setScrolled(window.scrollY > 12);
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
            <img src="/buglogin-logo.png" width="34" height="34" alt="" />
            <span>BugLogin</span>
          </a>
          <nav className={styles.nav} aria-label="Điều hướng chính">
            <a href="#profiles">Hồ sơ riêng</a>
            <a href="#features">Tính năng</a>
            <a href="#start">Bắt đầu</a>
            <a href="#faq">Hỏi đáp</a>
          </nav>
          <a className={styles.headerDownload} href="#download">
            Tải xuống
          </a>
        </div>
      </header>

      <main id="main">
        <section className={styles.hero} id="top" aria-labelledby="hero-title">
          <div className={styles.heroGrid} aria-hidden="true" />
          <div className={styles.heroInner}>
            <p className={styles.eyebrow}>
              TRÌNH DUYỆT DÀNH CHO CÔNG VIỆC NHIỀU TÀI KHOẢN
            </p>
            <h1 id="hero-title">
              Mỗi tài khoản.
              <br />
              <span className={styles.heroAccent}>Một hồ sơ riêng.</span>
            </h1>
            <p className={styles.lede}>
              Tách dữ liệu trình duyệt theo hồ sơ, quản lý quyền trong workspace
              và tiếp tục công việc trên thiết bị được cấp quyền.
            </p>
            <div className={styles.heroActions} id="download">
              <PlatformDownloads />
              <a className={styles.textLink} href="/login">
                Đăng nhập vào BugLogin{" "}
                <span className={styles.textLinkArrow} aria-hidden="true">
                  →
                </span>
              </a>
            </div>
            <div className={styles.heroVisual} aria-hidden="true">
              <div className={styles.browserFrame}>
                <div className={styles.browserBar}>
                  <span className={styles.browserDot} />
                  <span className={styles.browserDot} />
                  <span className={styles.browserDot} />
                  <div className={styles.browserAddress}>
                    <i /> Hồ sơ trình duyệt
                  </div>
                </div>
                <div className={styles.browserBody}>
                  <div className={styles.profileGlyph}>
                    <b />
                    <b />
                    <b />
                  </div>
                  <div className={styles.visualLines}>
                    <i />
                    <i />
                    <i />
                  </div>
                  <div className={styles.visualTag}>
                    Dữ liệu hồ sơ được lưu riêng
                  </div>
                </div>
              </div>
              <div className={styles.visualNote}>
                <span className={styles.visualNoteDot} /> Lưu trữ theo từng
                profile
              </div>
            </div>
          </div>
          <a
            className={styles.scrollCue}
            href="#profiles"
            aria-label="Cuộn đến phần tách hồ sơ"
          >
            ↓
          </a>
        </section>

        <section
          className={`${styles.section} ${styles.paper}`}
          id="profiles"
          aria-labelledby="profiles-title"
        >
          <div className={styles.wrap}>
            <div className={styles.splitSection}>
              <div>
                <p className={styles.eyebrowDark}>DỮ LIỆU CÓ RANH GIỚI</p>
                <h2 id="profiles-title">
                  Công việc tách biệt theo từng hồ sơ.
                </h2>
                <p className={styles.sectionLead}>
                  Cookie, phiên đăng nhập và dữ liệu trang web thuộc về hồ sơ
                  tương ứng. Bạn có thể đồng bộ hồ sơ qua dịch vụ BugLogin khi
                  tài khoản và workspace được cấp quyền.
                </p>
              </div>
              <div
                className={styles.isolationDiagram}
                role="img"
                aria-label="Ba hồ sơ có các vùng dữ liệu riêng"
              >
                <div className={styles.dataColumn}>
                  <span className={styles.dataColumnLabel}>Hồ sơ A</span>
                  <i />
                  <i />
                  <i />
                </div>
                <div className={styles.dataColumn}>
                  <span className={styles.dataColumnLabel}>Hồ sơ B</span>
                  <i />
                  <i />
                  <i />
                </div>
                <div className={styles.dataColumn}>
                  <span className={styles.dataColumnLabel}>Hồ sơ C</span>
                  <i />
                  <i />
                  <i />
                </div>
                <div className={styles.diagramCaption}>
                  Mỗi vùng dữ liệu thuộc một profile
                </div>
              </div>
            </div>
          </div>
        </section>

        <section
          className={`${styles.section} ${styles.white}`}
          id="features"
          aria-labelledby="features-title"
        >
          <div className={styles.wrap}>
            <header className={styles.sectionHead}>
              <p className={styles.eyebrowDark}>CÔNG CỤ CHO NHÓM VẬN HÀNH</p>
              <h2 id="features-title">Quản lý hồ sơ rõ ràng, từ một nơi.</h2>
            </header>
            <div className={styles.featureGrid}>
              <article>
                <span className={styles.featureIcon}>01</span>
                <h3>Hồ sơ riêng</h3>
                <p>
                  Mỗi profile sử dụng một thư mục dữ liệu riêng trên thiết bị.
                </p>
              </article>
              <article>
                <span className={styles.featureIcon}>02</span>
                <h3>Quyền theo vai trò</h3>
                <p>
                  Account, tổ chức, workspace, nhóm và profile được quản lý theo
                  quyền được cấp.
                </p>
              </article>
              <article>
                <span className={styles.featureIcon}>03</span>
                <h3>Đồng bộ có kiểm soát</h3>
                <p>
                  Đồng bộ dữ liệu hồ sơ khi dịch vụ lưu trữ và quyền truy cập
                  sẵn sàng.
                </p>
              </article>
              <article>
                <span className={styles.featureIcon}>04</span>
                <h3>REST API và MCP</h3>
                <p>Tự động hóa theo quyền của account đã cấp API key.</p>
              </article>
            </div>
          </div>
        </section>

        <section
          className={`${styles.section} ${styles.paper}`}
          id="start"
          aria-labelledby="start-title"
        >
          <div className={styles.wrap}>
            <header className={styles.sectionHead}>
              <p className={styles.eyebrowDark}>BẮT ĐẦU</p>
              <h2 id="start-title">Từ cài đặt đến hồ sơ đầu tiên.</h2>
            </header>
            <ol className={styles.steps}>
              <li>
                <b>01</b>
                <h3>Tải ứng dụng</h3>
                <p>Chọn đúng gói cài đặt cho Windows hoặc macOS.</p>
              </li>
              <li>
                <b>02</b>
                <h3>Đăng nhập</h3>
                <p>
                  Chọn tổ chức và workspace mà tài khoản của bạn truy cập được.
                </p>
              </li>
              <li>
                <b>03</b>
                <h3>Tạo hồ sơ</h3>
                <p>
                  Cấu hình profile, sau đó mở và quản lý dữ liệu của hồ sơ đó.
                </p>
              </li>
            </ol>
          </div>
        </section>

        <section
          className={`${styles.section} ${styles.white}`}
          id="faq"
          aria-labelledby="faq-title"
        >
          <div className={`${styles.wrap} ${styles.faqWrap}`}>
            <p className={styles.eyebrowDark}>HỎI ĐÁP</p>
            <h2 id="faq-title">Thông tin cần biết.</h2>
            <details>
              <summary>
                Dữ liệu profile được lưu ở đâu?<span>+</span>
              </summary>
              <p>
                Dữ liệu trình duyệt được lưu trong vùng dữ liệu riêng của
                profile trên thiết bị. Nếu bật đồng bộ và có quyền phù hợp, dữ
                liệu có thể được truyền qua dịch vụ BugLogin.
              </p>
            </details>
            <details>
              <summary>
                Tôi có thể dùng profile trên nhiều thiết bị không?<span>+</span>
              </summary>
              <p>
                Có thể dùng profile trên thiết bị khác khi cùng tài khoản có
                quyền truy cập và bản đồng bộ cần thiết đã tải hoàn tất. Một số
                dữ liệu phiên có thể phụ thuộc hệ điều hành hoặc runtime.
              </p>
            </details>
            <details>
              <summary>
                BugLogin hỗ trợ hệ điều hành nào?<span>+</span>
              </summary>
              <p>
                Trang tải sẽ hiển thị các bản phát hành ổn định hiện có cho
                Windows và macOS. Gói macOS hiện được xây dựng cho Apple
                Silicon.
              </p>
            </details>
            <details>
              <summary>
                API key MCP dùng quyền của ai?<span>+</span>
              </summary>
              <p>
                MCP thực hiện thao tác theo account và quyền của API key đã cấp.
                Hãy tạo key trong account cần kết nối và chỉ cấp scope cần
                thiết.
              </p>
            </details>
          </div>
        </section>

        <section className={styles.final} aria-labelledby="final-title">
          <div className={styles.finalInner}>
            <p className={styles.eyebrow}>BUGLOGIN DESKTOP</p>
            <h2 id="final-title">Bắt đầu với những hồ sơ tách biệt.</h2>
            <p>Chọn bản phát hành hiện có cho thiết bị của bạn.</p>
            <PlatformDownloads />
          </div>
        </section>
      </main>

      <footer className={styles.footer}>
        <div className={styles.footerInner}>
          <a className={styles.brand} href="#top">
            <img src="/buglogin-logo.png" width="30" height="30" alt="" />
            <span>BugLogin</span>
          </a>
          <nav aria-label="Liên kết cuối trang">
            <a href="/security">Bảo mật</a>
            <a href="/docs">Tài liệu</a>
            <a href="/download">Tải về</a>
            <a href="mailto:support@buglogin.com">Liên hệ</a>
          </nav>
          <small>© {new Date().getFullYear()} BugLogin</small>
        </div>
      </footer>
    </div>
  );
}
