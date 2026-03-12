const localFile = (name: string) => new URL(`./node_modules/${name}`, import.meta.url).pathname

export default {
  frontend: {
    entry: "src/client/main.tsx"
  },
  backend: {
    port: 3333,
    devCommand: {
      command: "cargo watch -x 'run -p vane'",
      cwd: "../../.."
    }
  },
  build: {
    pagesDir: "src/pages",
    outDir: ".seam/output",
    backendBuildCommand: {
      command: "cargo build -p vane --release",
      cwd: "../../.."
    },
    manifestCommand: {
      command: "cargo run -p vane -- --manifest",
      cwd: "../../.."
    }
  },
  generate: {
    manifestUrl: "http://127.0.0.1:3333/_seam/manifest.json"
  },
  vite: {
    resolve: {
      alias: [
        {
          find: /^@canmi\/seam-client$/,
          replacement: localFile("@canmi/seam-client/dist/index.js")
        },
        {
          find: /^@canmi\/seam-react$/,
          replacement: localFile("@canmi/seam-react/dist/index.js")
        },
        {
          find: /^@canmi\/seam-router$/,
          replacement: localFile("@canmi/seam-router/dist/index.js")
        },
        {
          find: /^@canmi\/seam-tanstack-router\/routes$/,
          replacement: localFile("@canmi/seam-tanstack-router/dist/define-routes.js")
        }
      ]
    },
    optimizeDeps: {
      include: ["@canmi/seam-tanstack-router"]
    }
  }
}
