import { AgentBrowser } from './lib/agent-browser.ts';
import { signIn } from './lib/auth.ts';

const DESKTOP = '/Users/yuniorrodriguezosorio/Desktop/origna-audit-2026-03-31';
const b = new AgentBrowser({ headed: false });

async function screenshot(name: string) {
  await b.screenshot(`${DESKTOP}/${name}.png`);
  console.log(`  ✓ ${name}`);
}

try {
  // Ensure output directory
  const { mkdirSync } = await import('node:fs');
  mkdirSync(DESKTOP, { recursive: true });

  // 1. Home page (unauthenticated)
  await b.open('https://dev.orignagta.ca');
  await b.waitForFlutter();
  await b.click('e1'); // Enable accessibility
  await new Promise(r => setTimeout(r, 3000));
  await screenshot('01-home-unauth');

  // 2. Login via API + reload
  const auth = await signIn('e2e-buyer@test.origna.ca');
  await b.open(`https://dev.orignagta.ca`);
  await b.waitForFlutter();
  await b.click('e1');
  await new Promise(r => setTimeout(r, 2000));

  // Store token in localStorage
  await b.evaluate(`localStorage.setItem('ob_access_token', '${auth.idToken}'); localStorage.setItem('ob_refresh_token', '${auth.refreshToken}'); location.reload();`);
  await new Promise(r => setTimeout(r, 4000));
  await screenshot('02-home-auth');

  // 3. Navigate pages via URL
  const pages = [
    ['03-notifications', '/notifications'],
    ['04-cart', '/cart'],
    ['05-profile', '/profile'],
    ['06-orders', '/orders'],
    ['07-favorites', '/favorites'],
  ];

  for (const [name, path] of pages) {
    await b.open(`https://dev.orignagta.ca/#${path}`);
    await new Promise(r => setTimeout(r, 3000));
    await screenshot(name);
  }

  console.log('All screenshots captured!');
} catch (e) {
  console.error('Error:', (e as Error).message);
} finally {
  await b.close();
}
