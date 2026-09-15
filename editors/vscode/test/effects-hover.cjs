const assert = require('node:assert/strict');
const { test } = require('node:test');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const { transformSync } = require('esbuild');

async function activate() {
  const commands = new Map(), requests = [], executed = [], warnings = [];
  let middleware, changedDocument, changedEditor, changedSelection;
  class Position {
    constructor(line, character) { Object.assign(this, { line, character }); }
    isEqual(other) { return this.line === other.line && this.character === other.character; }
  }
  class Range { constructor(start, end) { Object.assign(this, { start, end }); } }
  class Selection extends Range {
    constructor(start, end) { super(start, end); this.active = end; }
    isEqual(other) { return this.start.isEqual(other.start) && this.end.isEqual(other.end); }
  }
  const editor = {
    document: { uri: { toString: () => 'file:///navigation.cpp' }, version: 1 },
    selection: new Selection(new Position(0, 0), new Position(0, 0)),
  };
  const vscode = {
    Position, Range, Selection,
    MarkdownString: class { constructor(value) { this.value = value; } },
    Hover: class { constructor(contents, range) { Object.assign(this, { contents, range }); } },
    workspace: {
      getConfiguration: () => ({ get: (key, fallback) =>
        key === 'serverCommand' ? ['webspec-index', 'lsp'] : fallback }),
      onDidChangeTextDocument: callback => { changedDocument = callback; return {}; },
    },
    window: {
      activeTextEditor: editor,
      createOutputChannel: () => ({ appendLine() {} }),
      showWarningMessage: message => warnings.push(message),
      onDidChangeActiveTextEditor: callback => { changedEditor = callback; return {}; },
      onDidChangeTextEditorSelection: callback => { changedSelection = callback; return {}; },
    },
    commands: {
      registerCommand: (name, callback) => { commands.set(name, callback); return {}; },
      executeCommand: async (...args) => executed.push(args),
    },
  };
  const languageClient = { LanguageClient: class {
    constructor(_name, _label, _server, options) { middleware = options.middleware; }
    async start() {}
    sendRequest(method, params) {
      return new Promise((resolve, reject) => requests.push({ method, params, resolve, reject }));
    }
  }};
  const module = { exports: {} };
  const code = transformSync(fs.readFileSync(path.join(__dirname, '../src/extension.ts'), 'utf8'),
    { loader: 'ts', format: 'cjs', target: 'node20' }).code;
  vm.runInNewContext(code, {
    module, exports: module.exports,
    require: name => name === 'vscode' ? vscode :
      name === 'vscode-languageclient/node' ? languageClient : require(name),
  });
  await module.exports.activate({ subscriptions: [] });
  return {
    click: commands.get('webspecLens.showEffects'),
    toggle: commands.get('webspecLens.toggleEffectDetails'), requests, executed, warnings, editor,
    request: category => ({ category, document_uri: editor.document.uri.toString(),
      document_version: 1, source_position: { line: 12, character: 2 } }),
    hover: (document = editor.document, position = editor.selection.active) =>
      middleware.provideHover(document, position, {}, () => 'ordinary hover'),
    edit() { editor.document.version++; changedDocument({ document: editor.document }); },
    move() { editor.selection = new Selection(new Position(20, 0), new Position(20, 0));
      changedSelection({ textEditor: editor }); },
    switchEditor() { vscode.window.activeTextEditor = undefined; changedEditor(); },
    Position,
  };
}

test('category clicks use prepared details in a native hover at the step; latest click wins', async () => {
  const ui = await activate();
  const first = ui.click(ui.request('async'));
  const second = ui.click(ui.request('events'));
  assert.equal(ui.requests.length, 2);
  assert.ok(ui.requests.every(request => request.method === 'webspec/preparedEffectDocument'));
  ui.requests[1].resolve({ effect_details: [{ headline: 'may fire navigate event', markdown: '**Trace and conditions**\n\n1. Endpoint' }] });
  await second;
  ui.requests[0].resolve({ effect_details: [{ headline: 'Old result', markdown: 'old' }] });
  await first;
  assert.match(ui.hover().contents.value, /▸ may fire navigate event/);
  assert.doesNotMatch(ui.hover().contents.value, /Trace and conditions|Endpoint/);
  assert.doesNotMatch(ui.hover().contents.value, /<\/?details>|<summary>/);
  assert.equal(JSON.stringify(ui.hover().contents.isTrusted), JSON.stringify({ enabledCommands: ['webspecLens.toggleEffectDetails'] }));
  assert.equal(ui.editor.selection.active.line, 12);
  assert.equal(ui.editor.selection.active.character, 2);
  assert.deepEqual(ui.executed.map(args => args[0]), ['editor.action.hideHover', 'editor.action.showHover']);
  assert.equal(ui.executed[1][1].focus, 'autoFocusImmediately');
  assert.equal(ui.hover(undefined, new ui.Position(1, 0)), 'ordinary hover');
  const third = ui.click(ui.request('script'));
  ui.requests[2].resolve({ effect_details: [{ headline: 'may run script', markdown: 'Script endpoint' }] });
  await third;
  assert.match(ui.hover().contents.value, /▸ may run script/);
  assert.ok(ui.executed.every(args => args[0].startsWith('editor.action.')));
  assert.deepEqual(ui.warnings, []);
});

test('leaving the step, changing files or editing clears expanded hover details', async () => {
  for (const action of ['move', 'switchEditor', 'edit']) {
    const ui = await activate();
    const pending = ui.click(ui.request('events'));
    ui.requests[0].resolve({ effect_details: [{ headline: 'may fire navigate event', markdown: 'Event endpoint' }] });
    await pending;
    ui[action]();
    assert.equal(ui.hover(), 'ordinary hover');
  }
});

test('late results cannot steal focus after the user moves, switches files or edits', async () => {
  for (const action of ['move', 'switchEditor', 'edit']) {
    const ui = await activate();
    const pending = ui.click(ui.request('events'));
    ui[action]();
    ui.requests[0].resolve({ effect_details: [{ headline: 'may fire navigate event', markdown: 'Event endpoint' }] });
    await pending;
    assert.equal(ui.executed.length, 0);
    assert.equal(ui.hover(), 'ordinary hover');
  }
});

test('missing prepared data does not trigger analysis or open a document', async () => {
  const ui = await activate();
  const pending = ui.click(ui.request('events'));
  ui.requests[0].reject('Prepared paths unavailable');
  await pending;
  assert.equal(ui.requests.length, 1);
  assert.equal(ui.executed.length, 0);
  assert.match(ui.warnings[0], /Prepared paths unavailable/);
  await ui.click();
  assert.equal(ui.executed[0][0], 'editor.action.showHover');
});

function toggles(markdown) {
  return [...markdown.matchAll(/command:webspecLens\.toggleEffectDetails\?([^)]*)/g)]
    .map(match => JSON.parse(decodeURIComponent(match[1])));
}

test('headlines independently expand and collapse prepared details without another request', async () => {
  const ui = await activate();
  const pending = ui.click(ui.request('events'));
  ui.requests[0].resolve({ effect_details: [
    { headline: 'may fire navigate event', markdown: '**Navigate endpoint**' },
    { headline: 'may fire navigateerror event', markdown: '**Error endpoint**' },
  ] });
  await pending;
  const initial = ui.hover().contents.value;
  assert.match(initial, /may fire navigate event/);
  assert.match(initial, /may fire navigateerror event/);
  assert.doesNotMatch(initial, /endpoint/);
  const links = toggles(initial);
  assert.equal(links.length, 2);
  await ui.toggle(...links[0]);
  assert.match(ui.hover().contents.value, /▾ may fire navigate event/);
  assert.match(ui.hover().contents.value, /Navigate endpoint/);
  assert.doesNotMatch(ui.hover().contents.value, /Error endpoint/);
  await ui.toggle(...links[1]);
  assert.match(ui.hover().contents.value, /Error endpoint/);
  await ui.toggle(...links[0]);
  assert.doesNotMatch(ui.hover().contents.value, /Navigate endpoint/);
  assert.match(ui.hover().contents.value, /Error endpoint/);
  await ui.toggle(...links[1]);
  assert.equal(ui.hover().contents.value, initial);
  assert.equal(ui.requests.length, 1, 'expanding and collapsing must use already loaded details');
  for (const bad of [-1, 100, 0.5, '0']) {
    await ui.toggle(links[0][0], bad);
    assert.equal(ui.hover().contents.value, initial);
  }
  const next = ui.click(ui.request('script'));
  ui.requests[1].resolve({ effect_details: [{ headline: 'may run script', markdown: 'Script endpoint' }] });
  await next;
  const replaced = ui.hover().contents.value;
  await ui.toggle(...links[0]);
  assert.equal(ui.hover().contents.value, replaced, 'old hover links must not toggle a newer result');
});

test('effect names are escaped and only the local toggle command is trusted', async () => {
  const ui = await activate();
  const pending = ui.click(ui.request('events'));
  ui.requests[0].resolve({ effect_details: [{
    headline: 'may fire [name](command:evil) <b>event</b>', markdown: '[source](command:evil)',
  }] });
  await pending;
  const markdown = ui.hover().contents;
  assert.match(markdown.value, /\\\[name\\\]/);
  assert.equal(JSON.stringify(markdown.isTrusted), JSON.stringify({ enabledCommands: ['webspecLens.toggleEffectDetails'] }));
  assert.equal(toggles(markdown.value).length, 1);
});
