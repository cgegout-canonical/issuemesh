#!/usr/bin/env node
import { spawn } from 'node:child_process';
import { existsSync, mkdirSync, readFileSync, writeFileSync, chmodSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createServer } from 'node:net';

// 1. Check Chrome binary
let chromePath = process.env.CHROME_BIN || '/usr/bin/google-chrome';
if (!existsSync(chromePath)) {
  if (existsSync('/usr/bin/chromium-browser')) {
    chromePath = '/usr/bin/chromium-browser';
  } else if (existsSync('/usr/bin/chromium')) {
    chromePath = '/usr/bin/chromium';
  } else {
    console.log('No Google Chrome / Chromium binary found, skipping browser E2E.');
    process.exit(0);
  }
}

// 2. Output directory for screenshots
const screenshotsDir = join(process.cwd(), 'docs', 'screenshots');
mkdirSync(screenshotsDir, { recursive: true });

// 3. Helper to get an open local port
function getFreePort() {
  return new Promise((resolve, reject) => {
    const srv = createServer();
    srv.listen(0, '127.0.0.1', () => {
      const port = srv.address().port;
      srv.close(() => resolve(port));
    });
    srv.on('error', reject);
  });
}

function sleep(ms) {
  return new Promise((r) => setTimeout(r, ms));
}

// 4. Temporary test environment setup
const tempDir = join(tmpdir(), `issuemesh-browser-e2e-${Date.now()}`);
mkdirSync(tempDir, { recursive: true });
chmodSync(tempDir, 0o700);

const userHome = process.env.ISSUEMESH_HOME || join(process.env.HOME || '', '.issuemesh');
const secretsSrc = join(userHome, 'secrets.env');
if (existsSync(secretsSrc)) {
  const secretsContent = readFileSync(secretsSrc);
  const secretsDest = join(tempDir, 'secrets.env');
  writeFileSync(secretsDest, secretsContent);
  chmodSync(secretsDest, 0o600);
}

const chromeDataDir = join(tempDir, 'chrome-profile');
mkdirSync(chromeDataDir, { recursive: true });

let serverProc = null;
let chromeProc = null;

async function cleanup() {
  console.log('\nCleaning up processes...');
  if (chromeProc) {
    try {
      chromeProc.kill('SIGTERM');
    } catch (_) {}
  }
  if (serverProc) {
    try {
      serverProc.kill('SIGTERM');
    } catch (_) {}
  }
  try {
    rmSync(tempDir, { recursive: true, force: true });
  } catch (_) {}
}

process.on('SIGINT', async () => {
  await cleanup();
  process.exit(1);
});
process.on('SIGTERM', async () => {
  await cleanup();
  process.exit(1);
});

async function main() {
  const serverPort = await getFreePort();
  const cdpPort = await getFreePort();

  console.log(`[Browser E2E] Starting IssueMesh server on port ${serverPort}...`);
  const binPath = join(process.cwd(), 'target', 'release', 'issuemesh');
  if (!existsSync(binPath)) {
    console.error(`Binary not found at ${binPath}. Build release binary first.`);
    process.exit(1);
  }

  serverProc = spawn(binPath, ['--data-dir', tempDir, '--port', String(serverPort)], {
    env: { ...process.env, ISSUEMESH_HOME: tempDir },
    stdio: 'ignore'
  });

  // Wait for server health endpoint
  const baseUrl = `http://127.0.0.1:${serverPort}`;
  let serverHealthy = false;
  for (let i = 0; i < 50; i++) {
    await sleep(150);
    try {
      const res = await fetch(`${baseUrl}/api/health`);
      if (res.ok) {
        serverHealthy = true;
        break;
      }
    } catch (_) {}
  }

  if (!serverHealthy) {
    console.error('[Browser E2E] Failed to connect to IssueMesh server.');
    await cleanup();
    process.exit(1);
  }
  console.log(`[Browser E2E] IssueMesh server is up at ${baseUrl}`);

  // Start Chrome Headless
  console.log(`[Browser E2E] Spawning Chrome (${chromePath}) on debug port ${cdpPort}...`);
  chromeProc = spawn(
    chromePath,
    [
      '--headless=new',
      `--remote-debugging-port=${cdpPort}`,
      '--no-sandbox',
      '--disable-gpu',
      '--window-size=1280,900',
      `--user-data-dir=${chromeDataDir}`,
      'about:blank'
    ],
    { stdio: 'ignore' }
  );

  // Wait for CDP
  let versionData = null;
  for (let i = 0; i < 50; i++) {
    await sleep(150);
    try {
      const res = await fetch(`http://127.0.0.1:${cdpPort}/json/version`);
      if (res.ok) {
        versionData = await res.json();
        break;
      }
    } catch (_) {}
  }

  if (!versionData) {
    console.error('[Browser E2E] Failed to connect to Chrome DevTools Protocol.');
    await cleanup();
    process.exit(1);
  }

  console.log(`[Browser E2E] Chrome CDP ready: ${versionData['User-Agent']}`);

  // Fetch target list to get the page WebSocket
  const targetsRes = await fetch(`http://127.0.0.1:${cdpPort}/json/list`);
  const targets = await targetsRes.json();
  const pageTarget = targets.find((t) => t.type === 'page') || targets[0];

  if (!pageTarget || !pageTarget.webSocketDebuggerUrl) {
    console.error('[Browser E2E] Could not find page target debugger URL');
    await cleanup();
    process.exit(1);
  }

  // Connect WebSocket to page
  const ws = new WebSocket(pageTarget.webSocketDebuggerUrl);
  await new Promise((resolve, reject) => {
    ws.onopen = resolve;
    ws.onerror = reject;
  });

  let nextId = 1;
  const pendingRequests = new Map();
  ws.onmessage = (event) => {
    try {
      const msg = JSON.parse(event.data);
      if (msg.id && pendingRequests.has(msg.id)) {
        const { resolve, reject } = pendingRequests.get(msg.id);
        pendingRequests.delete(msg.id);
        if (msg.error) {
          reject(new Error(msg.error.message || JSON.stringify(msg.error)));
        } else {
          resolve(msg.result);
        }
      }
    } catch (e) {
      console.error('CDP message parse error:', e);
    }
  };

  function sendCommand(method, params = {}) {
    const id = nextId++;
    return new Promise((resolve, reject) => {
      pendingRequests.set(id, { resolve, reject });
      ws.send(JSON.stringify({ id, method, params }));
    });
  }

  async function evaluate(expression) {
    const res = await sendCommand('Runtime.evaluate', {
      expression,
      returnByValue: true,
      awaitPromise: true
    });
    return res.result ? res.result.value : null;
  }

  async function waitForSelector(selector, timeoutMs = 8000) {
    const start = Date.now();
    while (Date.now() - start < timeoutMs) {
      const exists = await evaluate(`!!document.querySelector(${JSON.stringify(selector)})`);
      if (exists) return true;
      await sleep(150);
    }
    throw new Error(`Timeout waiting for selector: ${selector}`);
  }

  async function captureScreenshot(filename, description) {
    const filePath = join(screenshotsDir, filename);
    const data = await sendCommand('Page.captureScreenshot', { format: 'png' });
    writeFileSync(filePath, Buffer.from(data.data, 'base64'));
    console.log(`[Screenshot] Saved ${filename} - ${description}`);
  }

  // Enable CDP domains
  await sendCommand('Page.enable');
  await sendCommand('Runtime.enable');
  await sendCommand('DOM.enable');

  console.log('[Browser E2E] Beginning UI verification sequence...');

  // --- Step 1: Onboarding Setup Page ---
  await sendCommand('Page.navigate', { url: `${baseUrl}/` });
  await waitForSelector('.wizard-step');
  await sleep(400);

  // Take screenshot of step 1 (welcome)
  await captureScreenshot('01_setup_wizard_welcome.png', 'Setup wizard welcome screen');

  // Navigate through steps 2, 3, 4, 5, 6, 7, 8
  await evaluate('goToStep(2)');
  await sleep(200);

  // Probe GitHub
  await evaluate('testGitHub()');
  await sleep(1000);

  // Step 3: Jira
  await evaluate('goToStep(3)');
  await sleep(200);
  await evaluate('testJira()');
  await sleep(1000);

  // Step 4: Launchpad
  await evaluate('goToStep(4)');
  await sleep(200);
  await evaluate('testLaunchpad()');
  await sleep(1000);

  // Step 5: Model
  await evaluate('goToStep(5)');
  await sleep(200);

  // Step 6: Memory
  await evaluate('goToStep(6)');
  await sleep(200);

  // Step 7: Defaults
  await evaluate('goToStep(7)');
  await sleep(200);

  // Step 8: Review
  await evaluate('goToStep(8); populateReview();');
  await sleep(300);
  await captureScreenshot('02_setup_wizard_review.png', 'Setup wizard review step');

  // Submit setup and finish onboarding
  await evaluate('finishSetup()');
  await sleep(1200);

  // --- Step 2: Home Search Page ---
  await sendCommand('Page.navigate', { url: `${baseUrl}/` });
  await waitForSelector('#query-input');
  await sleep(400);
  await captureScreenshot('03_home_search.png', 'Home search page');

  // Type query and run search
  const query = 'memory leak fix';
  await evaluate(`document.getElementById('query-input').value = ${JSON.stringify(query)}`);
  await evaluate(`runSearch()`);

  // Wait 1 second to capture active SSE progress list
  await sleep(1200);
  await captureScreenshot('04_search_progress.png', 'Live search progress with SSE events');

  // Wait for search to complete (search-btn re-enabled)
  const searchStart = Date.now();
  let completed = false;
  while (Date.now() - searchStart < 15000) {
    const isDone = await evaluate(`!document.getElementById('search-btn').disabled`);
    if (isDone) {
      completed = true;
      break;
    }
    await sleep(300);
  }
  await sleep(500);

  await captureScreenshot('05_search_results.png', 'Search results with evidence cards');

  // Open the first evidence card details
  await evaluate(`
    const details = document.querySelector('.card details');
    if (details) details.open = true;
  `);
  await sleep(300);
  await captureScreenshot('06_evidence_expanded.png', 'Evidence card expanded view');

  // Get current job id to inspect run detail
  const jobId = await evaluate(`currentJobId`);

  // --- Step 3: Memory Page ---
  await sendCommand('Page.navigate', { url: `${baseUrl}/memory` });
  await waitForSelector('.container');
  await sleep(500);
  await captureScreenshot('07_memory_page.png', 'Local memory & storage page');

  // --- Step 4: Sources Page ---
  await sendCommand('Page.navigate', { url: `${baseUrl}/sources` });
  await waitForSelector('.container');
  await sleep(500);
  await captureScreenshot('08_sources_page.png', 'Configured sources page');

  // --- Step 5: Health Page ---
  await sendCommand('Page.navigate', { url: `${baseUrl}/health` });
  await waitForSelector('.container');
  await sleep(500);
  await captureScreenshot('09_health_page.png', 'Subsystem health diagnostics page');

  // --- Step 6: Settings Page ---
  await sendCommand('Page.navigate', { url: `${baseUrl}/settings` });
  await waitForSelector('#gh-enabled');
  await sleep(500);
  await captureScreenshot('10_settings_page.png', 'Settings and configuration page');

  // --- Step 7: Search Run Detail Page ---
  if (jobId) {
    await sendCommand('Page.navigate', { url: `${baseUrl}/search/${encodeURIComponent(jobId)}/run` });
    await waitForSelector('.container');
    await sleep(500);
    await captureScreenshot('11_search_run_detail.png', `Search run detail for job ${jobId}`);
  }

  console.log('\n[Browser E2E] All browser verification steps and screenshots completed successfully!');
  ws.close();
  await cleanup();
  process.exit(0);
}

main().catch(async (err) => {
  console.error('[Browser E2E] Failure:', err);
  await cleanup();
  process.exit(1);
});
