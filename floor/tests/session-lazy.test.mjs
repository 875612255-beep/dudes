// A lazy chunk adds its own preload and stylesheet tags to the document after
// the page opened. They are not part of the build the page was loaded from.
import assert from 'node:assert/strict';
import test from 'node:test';

const BOOTSTRAP = `A${'b'.repeat(41)}E`;
const CSRF = `C${'d'.repeat(41)}I`;
const OWN = ['/floor/assets/index-aaaa1111.js', '/floor/assets/index-bbbb2222.css'];

test('a lazy chunk loaded after the page opened does not change which build the page is', async () => {
  const tag = (attribute, ref) => ({ getAttribute: (name) => (name === attribute ? ref : null) });
  const elements = [tag('src', OWN[0]), tag('href', OWN[1])];
  const location = { hash: `#bootstrap=${BOOTSTRAP}`, pathname: '/floor/', search: '' };
  const fetched = [];
  const window = {
    location,
    history: { replaceState() { location.hash = ''; } },
    fetch: async (url, init) => {
      fetched.push(`${init.method ?? 'GET'} ${url}`);
      if (init.method === 'GET') {
        const html = `<script type="module" crossorigin src="${OWN[0]}"></script><link rel="stylesheet" crossorigin href="${OWN[1]}">`;
        return { ok: true, status: 200, async text() { return html; } };
      }
      return { ok: true, status: 200, async json() { return { csrf: CSRF }; } };
    },
  };
  Object.assign(globalThis, { document: { querySelectorAll: () => elements }, window });
  try {
    const session = await import('../src/session.ts?loaded-with');
    elements.push(tag('href', '/floor/assets/grid-dddd4444.css'), tag('href', '/floor/assets/OfficeRenderer-eeee5555.js'));
    assert.deepEqual(await session.reauthenticateFloorSession(), { csrf: CSRF });
    assert.deepEqual(fetched, ['GET /floor/', 'POST /floor/session']);
  } finally {
    delete globalThis.document;
    delete globalThis.window;
  }
});
