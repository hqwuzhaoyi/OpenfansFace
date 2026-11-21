import type { NextConfig } from "next";

const nextConfig: NextConfig = {
  cacheComponents: false,
  output: "standalone",
  allowedDevOrigins: ["127.0.0.1", "localhost"],
  experimental: {
    proxyTimeout: 120_000,
  },
  typescript: {
    ignoreBuildErrors: process.env.NEXT_SKIP_TYPECHECK === "1",
  },
  turbopack: {
    rules: {
      "*.css": {
        loaders: ["@tailwindcss/turbopack"],
        as: "*.css",
      },
    },
  },
  async rewrites() {
    if (process.env.NODE_ENV === "production") return [];
    const remoteOrigin = process.env.DEV_REMOTE_ORIGIN?.replace(/\/$/, "");
    if (!remoteOrigin) return [];
    return {
      beforeFiles: [
        { source: "/api/:path*", destination: `${remoteOrigin}/api/:path*` },
        { source: "/thumb", destination: `${remoteOrigin}/thumb` },
      ],
    };
  },

};

export default nextConfig;
