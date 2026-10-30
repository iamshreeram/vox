import path from "node:path";
import { fileURLToPath } from "node:url";
import { defineConfig, devices } from "@playwright/test";

const here = path.dirname(fileURLToPath(import.meta.url));

// Dedicated config for the manual-test guide page. The page is opened from disk
// (file://), so there is deliberately NO webServer here: no process and no port
// is started. The repo's default Playwright config (testDir "./tests") never
// discovers this folder and CI does not run it.
export default defineConfig({
  testDir: path.join(here, "tests"),
  fullyParallel: false,
  workers: 1,
  reporter: "line",
  use: {
    // Optional: point at a system Chrome when Playwright's bundled browser is not installed.
    launchOptions: process.env.PW_CHROME_PATH
      ? { executablePath: process.env.PW_CHROME_PATH }
      : undefined,
  },
  projects: [{ name: "chromium", use: { ...devices["Desktop Chrome"] } }],
});
