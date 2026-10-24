import type { NextConfig } from "next";

const webHost = process.env.BUGLOGIN_APP_TARGET === "web";
const apiBaseUrl = process.env.BUGLOGIN_API_BASE_URL || (process.env.BUGLOGIN_ENV === "production" ? "" : "http://127.0.0.1:12343");

if (webHost && process.env.BUGLOGIN_ENV === "production" && !apiBaseUrl) {
  throw new Error("BUGLOGIN_API_BASE_URL is required for production web hosting");
}

const nextConfig: NextConfig = {
  reactStrictMode: true,
  output: webHost ? "standalone" : "export",
  images: {
    unoptimized: true,
  },
  distDir: webHost ? ".next-web" : "dist",
  compiler: {
    removeConsole: process.env.NODE_ENV === "production",
  },
  ...(webHost
    ? {
        experimental: {
          cpus: Number(process.env.BUGLOGIN_BUILD_CPUS || 2),
        },
        async rewrites() {
          return {
            beforeFiles: [
              ...(apiBaseUrl ? [{ source: "/v1/:path*", destination: `${apiBaseUrl}/v1/:path*` }] : []),
              { source: "/account/:path*", destination: "/" },
              { source: "/admin/:path*", destination: "/" },
              { source: "/workspaces/:path*", destination: "/" },
            ],
            afterFiles: [],
            fallback: [],
          };
        },
      }
    : {}),
};

export default nextConfig;
