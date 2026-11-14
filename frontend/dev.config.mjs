// Dev servers of the browser portals only: forwards the portal API to a
// running Prysm, so the page works with hot reload. Start a host first, e.g.
//   cargo run -p prysm-headless -- hosts/headless/settings.example.json
// PRYSM names another one (the TV: PRYSM=http://192.168.1.5:8787).
const target = process.env.PRYSM ?? 'http://127.0.0.1:8787';

/** The page's origin is the dev server's, which the portal would refuse. */
function sameOrigin(proxy) {
  proxy.on('proxyReq', (request) => request.removeHeader('origin'));
}

export default {
  server: {
    proxy: {
      '/api': { target, changeOrigin: true, configure: sameOrigin },
    },
  },
};
