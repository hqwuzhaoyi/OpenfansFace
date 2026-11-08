import type { NextConfig } from "next";

const nextConfig: NextConfig = {
  cacheComponents: false,
  output: "standalone",
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

};

export default nextConfig;
