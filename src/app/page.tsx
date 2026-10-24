import { WebRouteHost } from "./web-route-host";

export const metadata = {
  title: "BugLogin | Mỗi tài khoản, một hồ sơ riêng",
  description:
    "Tách dữ liệu trình duyệt theo hồ sơ, quản lý quyền trong workspace và tải bản BugLogin mới nhất cho Windows hoặc macOS.",
  metadataBase: new URL("https://buglogin.com"),
  openGraph: {
    title: "BugLogin | Mỗi tài khoản, một hồ sơ riêng",
    description:
      "Quản lý các profile trình duyệt trong một ứng dụng dành cho Windows và macOS.",
    url: "https://buglogin.com/",
    type: "website",
  },
};

export default function Home() {
  return <WebRouteHost />;
}
