(() => {
  'use strict';

  const root = document.documentElement;
  const body = document.body;
  const content = document.getElementById('content');
  const search = document.getElementById('vault-search');
  const tree = document.getElementById('vault-tree');
  const searchHint = document.getElementById('search-hint');
  const config = window.__VAULTLITE_CONFIG__ || {};
  const BINDINGS = config.bindings || {};
  const vaultName = config.vaultName || '';
  const runtimeToken = config.token || '';
  const isMac = /Mac|iPhone|iPad|iPod/i.test(navigator.platform);
  const mobileQuery = window.matchMedia('(max-width: 760px)');

  const store = {
    get(key) {
      try { return localStorage.getItem('vaultlite-' + key); } catch (_) { return null; }
    },
    set(key, value) {
      try { localStorage.setItem('vaultlite-' + key, value); } catch (_) { /* private mode */ }
    },
    json(key, fallback) {
      try {
        const raw = localStorage.getItem('vaultlite-' + key);
        return raw ? JSON.parse(raw) : fallback;
      } catch (_) { return fallback; }
    }
  };
  const clamp = (value, min, max) => Math.max(min, Math.min(max, value));
  const isEditing = element => element instanceof Element &&
    !!element.closest('input, textarea, select, [contenteditable="true"]');

  /* Toast feedback --------------------------------------------------------- */

  const toastElement = document.getElementById('toast');
  let toastTimer = 0;
  const showToast = (message, error = false) => {
    if (!toastElement) return;
    window.clearTimeout(toastTimer);
    toastElement.textContent = message;
    toastElement.dataset.kind = error ? 'error' : 'info';
    toastElement.hidden = false;
    toastTimer = window.setTimeout(() => { toastElement.hidden = true; }, 2800);
  };

  const currentNotePath = () => {
    const path = location.pathname;
    if (path.startsWith('/note/')) return path.slice('/note/'.length);
    if (path.startsWith('/file/') && path.toLowerCase().endsWith('.md')) return path.slice('/file/'.length);
    return null;
  };

  const apiPost = (path, body) => fetch(path, {
    method: 'POST',
    headers: {
      'X-VaultLite-Token': runtimeToken,
      ...(body ? { 'Content-Type': 'application/json' } : {})
    },
    body: body ? JSON.stringify(body) : undefined
  });

  const copyText = async text => {
    if (navigator.clipboard && navigator.clipboard.writeText) {
      await navigator.clipboard.writeText(text);
      return;
    }
    const area = document.createElement('textarea');
    area.value = text;
    area.setAttribute('readonly', '');
    area.style.position = 'fixed';
    area.style.opacity = '0';
    document.body.append(area);
    area.select();
    document.execCommand('copy');
    area.remove();
  };

  const copyNote = async () => {
    const encoded = currentNotePath();
    if (!encoded) {
      showToast('Open a Markdown note before copying it.', true);
      return;
    }
    try {
      const response = await fetch('/raw/' + encoded, { cache: 'no-store' });
      if (!response.ok) throw new Error('Unable to read note');
      await copyText(await response.text());
      showToast('Note copied to clipboard.');
    } catch (_) {
      showToast('Could not copy this note.', true);
    }
  };

  const openInObsidian = () => {
    const encoded = currentNotePath();
    if (!encoded) {
      showToast('Open a Markdown note before using Obsidian.', true);
      return;
    }
    if (!vaultName) {
      showToast('Vault name is unavailable.', true);
      return;
    }
    let file = encoded;
    try { file = decodeURIComponent(encoded); } catch (_) { /* keep encoded */ }
    file = file.replace(/\.md$/i, '');
    const uri = 'obsidian://open?vault=' + encodeURIComponent(vaultName) + '&file=' + encodeURIComponent(file);
    window.location.href = uri;
    showToast('Opening note in Obsidian…');
  };

  const exitApp = async () => {
    if (!window.confirm('Exit VaultLite and stop the local server?')) return;
    try {
      await apiPost('/api/quit');
    } catch (_) { /* the server may stop before the response is read */ }
    document.body.innerHTML = '<main class="exit-screen"><h1>VaultLite has stopped</h1><p>You can close this tab.</p></main>';
  };

  const openConfigFolder = async () => {
    try {
      const response = await apiPost('/api/open-config-folder');
      if (!response.ok) throw new Error('Unable to open folder');
      showToast('Opened the VaultLite config folder.');
    } catch (_) {
      showToast('Could not open the config folder.', true);
    }
  };

  /* Theme and color palettes ------------------------------------------------ */

  const paletteVars = ['bg', 'panel', 'fg', 'muted', 'line', 'accent', 'code', 'hover', 'bold-color', 'italic-color', 'strike-color', 'link-color', 'h1-color', 'h2-color', 'h3-color', 'h4-color', 'h5-color', 'h6-color'];
  const palettes = {
    'default': { light: ['#ffffff', '#fafafa', '#222222', '#777777', '#e7e7e7', '#526fe5', '#f3f4f6', '#f1f3f8', '#334b91', '#8a4e91', '#888888', '#365fc8', '#334b91', '#3d5fa5', '#496da8', '#607da8', '#66788d', '#77808b'], dark: ['#171819', '#1d1f21', '#e6e6e6', '#a0a0a0', '#343638', '#9aafff', '#24272a', '#282b2e', '#b9c8ff', '#e2b8e8', '#999999', '#a8bbff', '#c2d0ff', '#b4c6f5', '#a9c0e9', '#a6b7d5', '#b2bfd1', '#c0c7d0'] },
    'nord': { light: ['#eceff4', '#e5e9f0', '#2e3440', '#616e88', '#d8dee9', '#5e81ac', '#e5e9f0', '#d8dee9', '#3b4252', '#b06b9d', '#7b8498', '#4c6f91', '#3b4252', '#4c566a', '#5e6b82', '#687a96', '#7888a0', '#8895a8'], dark: ['#2e3440', '#3b4252', '#eceff4', '#aeb9cc', '#4c566a', '#88c0d0', '#434c5e', '#434c5e', '#ebcb8b', '#b48ead', '#9aa5b8', '#88c0d0', '#88c0d0', '#81a1c1', '#8fbcbb', '#a3be8c', '#d08770', '#e5e9f0'] },
    'dracula': { light: ['#f8f8f2', '#eeeeeb', '#282a36', '#666477', '#dedee5', '#7652b5', '#eeeeeb', '#e3e1ef', '#59428c', '#a14e83', '#777777', '#6441a5', '#6441a5', '#7652b5', '#8b5fbf', '#a16ac0', '#78618d', '#686078'], dark: ['#282a36', '#343746', '#f8f8f2', '#a9a6b8', '#44475a', '#bd93f9', '#343746', '#44475a', '#f1fa8c', '#ff79c6', '#a9a6b8', '#8be9fd', '#ff79c6', '#bd93f9', '#8be9fd', '#50fa7b', '#ffb86c', '#f8f8f2'] },
    'catppuccin': { light: ['#eff1f5', '#e6e9ef', '#4c4f69', '#7c7f93', '#ccd0da', '#8839ef', '#e6e9ef', '#dce0e8', '#1e66f5', '#ea76cb', '#8c8fa1', '#1e66f5', '#8839ef', '#7287fd', '#04a5e5', '#179299', '#40a02b', '#df8e1d'], dark: ['#1e1e2e', '#181825', '#cdd6f4', '#a6adc8', '#45475a', '#cba6f7', '#313244', '#313244', '#f9e2af', '#f5c2e7', '#9399b2', '#89b4fa', '#cba6f7', '#b4befe', '#89dceb', '#94e2d5', '#a6e3a1', '#f9e2af'] },
    'gruvbox': { light: ['#fbf1c7', '#f2e5bc', '#3c3836', '#7c6f64', '#d5c4a1', '#af3a03', '#f2e5bc', '#ebdbb2', '#076678', '#8f3f71', '#928374', '#076678', '#9d0006', '#b57614', '#79740e', '#427b58', '#076678', '#8f3f71'], dark: ['#282828', '#3c3836', '#ebdbb2', '#a89984', '#504945', '#fabd2f', '#3c3836', '#504945', '#fabd2f', '#d3869b', '#928374', '#83a598', '#fb4934', '#fabd2f', '#b8bb26', '#8ec07c', '#83a598', '#d3869b'] },
    'solarized': { light: ['#fdf6e3', '#eee8d5', '#586e75', '#839496', '#ddd6c1', '#268bd2', '#eee8d5', '#eee8d5', '#b58900', '#d33682', '#93a1a1', '#268bd2', '#b58900', '#cb4b16', '#859900', '#2aa198', '#268bd2', '#6c71c4'], dark: ['#002b36', '#073642', '#eee8d5', '#93a1a1', '#586e75', '#268bd2', '#073642', '#073642', '#b58900', '#d33682', '#657b83', '#268bd2', '#b58900', '#cb4b16', '#859900', '#2aa198', '#268bd2', '#6c71c4'] },
    'tokyo-night': { light: ['#e1e2e7', '#d5d6db', '#343b58', '#697098', '#c4c8da', '#34548a', '#d5d6db', '#c8cbe0', '#34548a', '#9854a6', '#777f9b', '#34548a', '#34548a', '#485eaf', '#5a6da8', '#4c7899', '#6b6fa5', '#777f9b'], dark: ['#1a1b26', '#24283b', '#c0caf5', '#9aa5ce', '#3b4261', '#7aa2f7', '#24283b', '#292e42', '#e0af68', '#bb9af7', '#7982a9', '#7dcfff', '#7aa2f7', '#bb9af7', '#7dcfff', '#9ece6a', '#e0af68', '#c0caf5'] },
    'rose-pine': { light: ['#faf4ed', '#f2e9e1', '#575279', '#797593', '#dfdad9', '#907aa9', '#f2e9e1', '#f2e9e1', '#286983', '#d7827e', '#9893a5', '#56949f', '#907aa9', '#286983', '#ea9d34', '#56949f', '#d7827e', '#797593'], dark: ['#191724', '#1f1d2e', '#e0def4', '#908caa', '#403d52', '#c4a7e7', '#26233a', '#26233a', '#f6c177', '#ebbcba', '#6e6a86', '#c4a7e7', '#c4a7e7', '#9ccfd8', '#f6c177', '#ebbcba', '#31748f', '#e0def4'] }
  };
  const paletteMenu = document.getElementById('palette-menu');
  const paletteToggle = document.getElementById('palette-toggle');
  const themeButton = document.getElementById('theme-toggle');
  const themeIcon = document.getElementById('theme-icon');
  const activePalette = () => store.get('palette') || 'default';
  const applyPalette = name => {
    const palette = palettes[name] || palettes.default;
    const values = palette[root.dataset.theme === 'dark' ? 'dark' : 'light'];
    values.forEach((value, index) => root.style.setProperty('--' + paletteVars[index], value));
    store.set('palette', palettes[name] ? name : 'default');
    paletteMenu.querySelectorAll('[data-palette]').forEach(button => {
      button.setAttribute('aria-pressed', String(button.dataset.palette === name));
    });
  };
  const setTheme = theme => {
    root.dataset.theme = theme;
    store.set('theme', theme);
    const dark = theme === 'dark';
    if (themeIcon) themeIcon.setAttribute('href', dark ? '#i-sun' : '#i-moon');
    themeButton.title = dark ? 'Switch to light theme' : 'Switch to dark theme';
    themeButton.setAttribute('aria-label', themeButton.title);
    applyPalette(activePalette());
  };
  setTheme(root.dataset.theme === 'dark' ? 'dark' : 'light');

  const setPaletteMenu = open => {
    paletteMenu.hidden = !open;
    paletteToggle.setAttribute('aria-expanded', String(open));
  };
  paletteToggle.setAttribute('aria-expanded', 'false');
  paletteMenu.querySelectorAll('[data-palette]').forEach(button => button.addEventListener('click', () => {
    applyPalette(button.dataset.palette);
    setPaletteMenu(false);
  }));
  document.addEventListener('click', event => {
    if (!paletteMenu.hidden && !paletteMenu.contains(event.target) && !paletteToggle.contains(event.target)) setPaletteMenu(false);
  });

  /* Sidebars --------------------------------------------------------------- */

  const navBackdrop = document.getElementById('nav-backdrop');
  const leftSidebar = {
    hidden() { return root.dataset.navHidden === 'true'; },
    set(hidden) {
      root.dataset.navHidden = String(hidden);
      store.set('sidebar-hidden', hidden ? '1' : '0');
      this.sync();
    },
    toggle() { this.set(!this.hidden()); },
    sync() {
      const drawerOpen = mobileQuery.matches && !this.hidden();
      body.classList.toggle('nav-drawer-open', drawerOpen);
      if (navBackdrop) navBackdrop.hidden = !drawerOpen;
    }
  };
  const rightSidebar = {
    available() { return !!document.querySelector('.layout > aside'); },
    hidden() { return root.dataset.rightHidden === 'true'; },
    set(hidden) {
      root.dataset.rightHidden = String(hidden);
      store.set('right-sidebar-hidden', hidden ? '1' : '0');
      this.sync();
    },
    toggle() {
      if (!this.available()) return;
      this.set(!this.hidden());
    },
    sync() {
      const available = this.available();
      body.classList.toggle('has-toc', available);
      const button = document.getElementById('show-right-sidebar');
      if (button) button.hidden = !(available && this.hidden());
      const hide = document.getElementById('hide-right-sidebar');
      if (hide) hide.setAttribute('aria-expanded', String(!this.hidden()));
    }
  };
  leftSidebar.sync();
  rightSidebar.sync();
  if (navBackdrop) navBackdrop.addEventListener('click', () => leftSidebar.set(true));

  /* Right sidebar width and level-1-only mode ------------------------------- */

  let tocWidth = clamp(Number(store.get('toc-width')) || 240, 160, 520);
  const setTocWidth = value => {
    tocWidth = clamp(value, 160, 520);
    root.style.setProperty('--toc-width', tocWidth + 'px');
    store.set('toc-width', String(tocWidth));
  };
  setTocWidth(tocWidth);

  let tocLevelOne = store.get('toc-level-one') === '1';
  const syncTocControls = () => {
    document.querySelectorAll('.layout > aside').forEach(aside => aside.classList.toggle('toc-level-one', tocLevelOne));
    document.querySelectorAll('[data-toc-level-one]').forEach(button => {
      button.setAttribute('aria-pressed', String(tocLevelOne));
      button.title = tocLevelOne ? 'Show all headings' : 'Show only level 1 headings';
      button.setAttribute('aria-label', button.title);
    });
  };
  const setTocLevelOne = value => {
    tocLevelOne = !!value;
    store.set('toc-level-one', tocLevelOne ? '1' : '0');
    syncTocControls();
  };
  syncTocControls();

  /* File tree: compact, persisted folders, fuzzy filter --------------------- */

  const folders = Array.from(tree.querySelectorAll('details.folder'));
  const folderState = store.json('folder-state', {});
  folders.forEach(folder => {
    if (Object.prototype.hasOwnProperty.call(folderState, folder.dataset.path)) {
      folder.open = !!folderState[folder.dataset.path];
    }
    folder.addEventListener('toggle', saveFolderState);
  });
  function saveFolderState() {
    const state = {};
    folders.forEach(folder => { state[folder.dataset.path] = folder.open; });
    store.set('folder-state', JSON.stringify(state));
  }
  const collapseFolders = () => {
    folders.forEach(folder => { folder.open = false; });
    saveFolderState();
  };
  const expandFolders = () => {
    folders.forEach(folder => { folder.open = true; });
    saveFolderState();
  };

  /* Fuzzy search: matches the whole filename, tolerates typos, and ranks the
     best matches first instead of merely filtering the tree. */

  const normalizeSearchText = value => String(value || '')
    .normalize('NFD')
    .replace(/\p{Diacritic}/gu, '')
    .toLocaleLowerCase();

  const editDistanceWithin = (a, b, limit) => {
    if (a === b) return 0;
    if (Math.abs(a.length - b.length) > limit) return limit + 1;
    let previous = Array.from({ length: b.length + 1 }, (_, i) => i);
    for (let i = 1; i <= a.length; i++) {
      const current = Array(b.length + 1).fill(limit + 1);
      current[0] = i;
      const start = Math.max(1, i - limit);
      const end = Math.min(b.length, i + limit);
      let rowMin = current[0];
      for (let j = start; j <= end; j++) {
        current[j] = Math.min(current[j - 1] + 1, previous[j] + 1, previous[j - 1] + (a[i - 1] === b[j - 1] ? 0 : 1));
        rowMin = Math.min(rowMin, current[j]);
      }
      if (rowMin > limit) return limit + 1;
      previous = current;
    }
    return previous[b.length];
  };
  const subsequenceSpan = (needle, haystack) => {
    let at = 0;
    let start = -1;
    let end = -1;
    for (let i = 0; i < haystack.length && at < needle.length; i++) {
      if (haystack[i] === needle[at]) {
        if (start === -1) start = i;
        at++;
        end = i;
      }
    }
    return at === needle.length ? [start, end] : null;
  };
  const searchTokenScore = (token, text) => {
    if (!token) return 0;
    const direct = text.indexOf(token);
    if (direct !== -1) {
      const atWordStart = direct === 0 || !/[\p{L}\p{N}]/u.test(text[direct - 1]);
      return 90 + (atWordStart ? 35 : 0) + (token.length / Math.max(1, text.length)) * 50 - Math.min(direct, 40);
    }
    const span = subsequenceSpan(token, text);
    if (span) {
      const [start, end] = span;
      return 45 - (end - start - token.length) * 2.5 - start * 0.5;
    }
    const threshold = token.length < 4 ? 1 : token.length < 7 ? 2 : 3;
    let best = -Infinity;
    for (const word of text.split(/[^\p{L}\p{N}]+/u)) {
      if (!word || Math.abs(word.length - token.length) > threshold) continue;
      const distance = editDistanceWithin(token, word, threshold);
      if (distance <= threshold) best = Math.max(best, 25 - distance * 8);
    }
    return best;
  };
  const fuzzyScore = (query, name, path) => {
    const tokens = normalizeSearchText(query).split(/\s+/).filter(Boolean);
    if (!tokens.length) return 0;
    let total = 0;
    for (const token of tokens) {
      // Folder names are searchable too, but filename matches rank higher.
      const best = Math.max(searchTokenScore(token, name), searchTokenScore(token, path) - 15);
      if (best === -Infinity) return -Infinity;
      total += best;
    }
    return total;
  };

  const fileLinkByPath = new Map();
  const indexTreeOrder = container => {
    Array.from(container.children).forEach((child, index) => {
      child._vaultOrder = index;
      if (child.matches('details.folder')) {
        const children = child.querySelector(':scope > .folder-children');
        if (children) indexTreeOrder(children);
      }
    });
  };
  indexTreeOrder(tree);
  tree.querySelectorAll('a.file').forEach(node => {
    node._vaultName = normalizeSearchText(node.dataset.name);
    node._vaultPath = normalizeSearchText(node.dataset.path);
    let path = new URL(node.href, location.href).pathname;
    if (path.startsWith('/file/') && path.toLowerCase().endsWith('.md')) path = path.replace(/^\/file\//, '/note/');
    fileLinkByPath.set(path, node);
  });
  let activeFileLink = null;
  const syncActiveFile = () => {
    if (activeFileLink) activeFileLink.classList.remove('active');
    let current = location.pathname;
    if (current.startsWith('/file/') && current.toLowerCase().endsWith('.md')) current = current.replace(/^\/file\//, '/note/');
    activeFileLink = fileLinkByPath.get(current) || null;
    if (activeFileLink) {
      activeFileLink.classList.add('active');
      activeFileLink.scrollIntoView({ block: 'nearest' });
    }
  };
  const sortChildren = (container, byScore) => {
    const children = Array.from(container.children);
    children.sort((a, b) => {
      if (byScore) {
        const difference = (b._matchScore ?? -Infinity) - (a._matchScore ?? -Infinity);
        if (difference) return difference;
      }
      return a._vaultOrder - b._vaultOrder;
    });
    children.forEach(child => container.append(child));
  };
  const scoreNode = (node, query) => {
    if (node.matches('a.file')) {
      node._matchScore = query ? fuzzyScore(query, node._vaultName, node._vaultPath) : 0;
      node.style.display = !query || node._matchScore !== -Infinity ? '' : 'none';
      return node._matchScore;
    }
    if (node.matches('details.folder')) {
      const children = node.querySelector(':scope > .folder-children');
      let best = query ? -Infinity : 0;
      if (children) {
        Array.from(children.children).forEach(child => {
          const score = scoreNode(child, query);
          if (score > best) best = score;
        });
        sortChildren(children, !!query);
      }
      node._matchScore = best;
      node.style.display = !query || best !== -Infinity ? '' : 'none';
      if (query && best !== -Infinity) node.open = true;
      return best;
    }
    return query ? -Infinity : 0;
  };
  const applySearch = () => {
    const query = search.value.trim();
    Array.from(tree.children).forEach(node => scoreNode(node, query));
    sortChildren(tree, !!query);
  };
  let searchTimer = 0;
  search.addEventListener('input', () => {
    window.clearTimeout(searchTimer);
    searchTimer = window.setTimeout(applySearch, 45);
  });
  search.addEventListener('keydown', event => {
    if (event.key === 'Escape') {
      search.value = '';
      applySearch();
      search.blur();
    }
  });
  const openSearch = () => {
    leftSidebar.set(false);
    window.requestAnimationFrame(() => {
      search.focus();
      search.select();
    });
  };

  /* Wrapping, sidebar width, note text size --------------------------------- */

  const legacyLeftWrap = store.get('left-no-wrap');
  const legacyRightWrap = store.get('right-no-wrap');
  const initialWrap = (side, legacy) => {
    const stored = store.get(side + '-wrap');
    if (stored !== null) return stored === '1';
    if (legacy !== null) return legacy === '0';
    return false;
  };
  const getWrap = side => body.classList.contains(side + '-wrap');
  const applyWrap = (side, wrap) => {
    body.classList.toggle(side + '-wrap', wrap);
    document.querySelectorAll('[data-wrap-control="' + side + '"]').forEach(button => {
      button.setAttribute('aria-pressed', String(wrap));
      button.title = (wrap ? 'Disable' : 'Enable') + ' wrapping in the ' + (side === 'left' ? 'file navigator' : 'table of contents');
      button.setAttribute('aria-label', button.title);
    });
  };
  const setWrap = (side, wrap) => {
    applyWrap(side, wrap);
    store.set(side + '-wrap', wrap ? '1' : '0');
  };
  applyWrap('left', initialWrap('left', legacyLeftWrap));
  applyWrap('right', initialWrap('right', legacyRightWrap));

  let sidebarWidth = clamp(Number(store.get('sidebar-width')) || 270, 190, 520);
  const setSidebarWidth = value => {
    sidebarWidth = clamp(value, 190, 520);
    root.style.setProperty('--sidebar-width', sidebarWidth + 'px');
    store.set('sidebar-width', String(sidebarWidth));
  };
  setSidebarWidth(sidebarWidth);

  let noteFontSize = clamp(Number(store.get('note-font-size')) || 16, 12, 24);
  const setNoteFontSize = value => {
    noteFontSize = clamp(value, 12, 24);
    root.style.setProperty('--note-font-size', noteFontSize + 'px');
    store.set('note-font-size', String(noteFontSize));
    document.getElementById('font-smaller').title = 'Decrease note text size (' + noteFontSize + 'px)';
    document.getElementById('font-larger').title = 'Increase note text size (' + noteFontSize + 'px)';
  };
  setNoteFontSize(noteFontSize);

  /* Heading search --------------------------------------------------------- */

  const setupHeadingSearch = () => {
    const headingSearch = document.getElementById('heading-search');
    const toc = document.querySelector('.toc');
    if (!headingSearch || !toc) return;
    headingSearch.addEventListener('input', () => {
      const query = headingSearch.value.trim().toLocaleLowerCase();
      toc.querySelectorAll('a').forEach(link => {
        link.style.display = !query || link.textContent.toLocaleLowerCase().includes(query) ? '' : 'none';
      });
    });
    headingSearch.addEventListener('keydown', event => {
      if (event.key === 'Escape') {
        headingSearch.value = '';
        headingSearch.dispatchEvent(new Event('input'));
        headingSearch.blur();
      }
    });
  };
  setupHeadingSearch();

  /* Backlinks -------------------------------------------------------------- */

  let backlinkLoadToken = 0;
  const hydrateBacklinks = async encodedPath => {
    const token = ++backlinkLoadToken;
    const container = document.getElementById('backlinks');
    if (!container) return;
    const route = '/backlinks/' + encodedPath;
    for (let attempt = 0; attempt < 90; attempt++) {
      try {
        const response = await fetch(route, { cache: 'no-store' });
        const html = await response.text();
        if (token !== backlinkLoadToken || !document.getElementById('backlinks')) return;
        if (response.status === 202) {
          if (attempt % 5 === 0) container.innerHTML = html;
          await new Promise(resolve => setTimeout(resolve, 650));
          continue;
        }
        if (response.ok) container.outerHTML = html;
        else container.innerHTML = '<table class="link-relations"><tbody><tr><td>Unable to load links.</td></tr></tbody></table>';
        return;
      } catch (_) {
        if (token !== backlinkLoadToken) return;
        await new Promise(resolve => setTimeout(resolve, 1000));
      }
    }
    if (token === backlinkLoadToken && document.getElementById('backlinks')) {
      container.innerHTML = '<table class="link-relations"><tbody><tr><td>Link index is still being prepared.</td></tr></tbody></table>';
    }
  };

  /* Canvas boards ---------------------------------------------------------- */

  const canvasIconFor = file => {
    const extension = (file.split('.').pop() || '').toLowerCase();
    if (['png', 'jpg', 'jpeg', 'gif', 'svg', 'webp', 'avif', 'bmp', 'ico', 'tif', 'tiff'].includes(extension)) return 'i-image';
    if (extension === 'md') return 'i-file-text';
    if (extension === 'base') return 'i-table';
    if (extension === 'canvas') return 'i-canvas';
    return 'i-file';
  };
  const canvasHref = file => {
    const extension = (file.split('.').pop() || '').toLowerCase();
    const route = extension === 'md' ? 'note' : extension === 'base' ? 'base' : extension === 'canvas' ? 'canvas' :
      ['png', 'jpg', 'jpeg', 'gif', 'svg', 'webp', 'avif', 'bmp', 'ico', 'tif', 'tiff'].includes(extension) ? 'image' : 'file';
    return '/' + route + '/' + file.split('/').map(encodeURIComponent).join('%2F');
  };
  /* Canvas text nodes contain Markdown. Build DOM nodes rather than assigning
     innerHTML so note content can never inject markup. */
  const appendCanvasInline = (parent, text) => {
    const pattern = /(\*\*[^*\n]+\*\*|__[^_\n]+__|~~[^~\n]+~~|==[^=\n]+==|`[^`\n]+`|\*[^*\n]+\*|_[^_\n]+_|\[[^\]\n]+\]\([^)\n]+\))/g;
    let last = 0;
    let match;
    while ((match = pattern.exec(text))) {
      if (match.index > last) parent.append(document.createTextNode(text.slice(last, match.index)));
      const token = match[0];
      let element;
      if (token.startsWith('**') || token.startsWith('__')) {
        element = document.createElement('strong');
        element.textContent = token.slice(2, -2);
      } else if (token.startsWith('~~')) {
        element = document.createElement('del');
        element.textContent = token.slice(2, -2);
      } else if (token.startsWith('==')) {
        element = document.createElement('mark');
        element.textContent = token.slice(2, -2);
      } else if (token.startsWith('`')) {
        element = document.createElement('code');
        element.textContent = token.slice(1, -1);
      } else if (token.startsWith('[')) {
        const link = /^\[([^\]]+)\]\(([^)]+)\)$/.exec(token);
        element = document.createElement('a');
        element.textContent = link ? link[1] : token;
        if (link) {
          element.href = link[2];
          if (/^https?:/i.test(link[2])) {
            element.target = '_blank';
            element.rel = 'noopener';
          }
        }
      } else {
        element = document.createElement('em');
        element.textContent = token.slice(1, -1);
      }
      parent.append(element);
      last = match.index + token.length;
    }
    if (last < text.length) parent.append(document.createTextNode(text.slice(last)));
  };
  const renderCanvasMarkdown = (container, text) => {
    const lines = String(text || '').replace(/\r\n?/g, '\n').split('\n');
    let paragraph = [];
    let list = null;
    let code = null;
    const flushParagraph = () => {
      if (!paragraph.length) return;
      const element = document.createElement('p');
      appendCanvasInline(element, paragraph.join(' '));
      container.append(element);
      paragraph = [];
    };
    const flushList = () => {
      if (!list) return;
      container.append(list);
      list = null;
    };
    for (const line of lines) {
      if (code) {
        if (line.trim().startsWith('```')) {
          code.element.textContent = code.lines.join('\n');
          container.append(code.element);
          code = null;
        } else {
          code.lines.push(line);
        }
        continue;
      }
      if (line.trim().startsWith('```')) {
        flushParagraph();
        flushList();
        code = { element: document.createElement('pre'), lines: [] };
        continue;
      }
      if (!line.trim()) {
        flushParagraph();
        flushList();
        continue;
      }
      const heading = /^(#{1,6})\s+(.*)$/.exec(line);
      if (heading) {
        flushParagraph();
        flushList();
        const element = document.createElement('h' + heading[1].length);
        appendCanvasInline(element, heading[2]);
        container.append(element);
        continue;
      }
      const item = /^\s*[-*+]\s+(.*)$/.exec(line);
      if (item) {
        flushParagraph();
        if (!list) list = document.createElement('ul');
        const element = document.createElement('li');
        appendCanvasInline(element, item[1]);
        list.append(element);
        continue;
      }
      const quote = /^\s*>\s?(.*)$/.exec(line);
      if (quote) {
        flushParagraph();
        flushList();
        const element = document.createElement('blockquote');
        appendCanvasInline(element, quote[1]);
        container.append(element);
        continue;
      }
      paragraph.push(line.trim());
    }
    flushParagraph();
    flushList();
    if (code) {
      code.element.textContent = code.lines.join('\n');
      container.append(code.element);
    }
  };

  const renderCanvasBoard = (host, data) => {
    const nodes = (Array.isArray(data.nodes) ? data.nodes : []).filter(node => node && typeof node === 'object' && node.id);
    const edges = (Array.isArray(data.edges) ? data.edges : []).filter(edge => edge && typeof edge === 'object');
    host.replaceChildren();
    const positioned = nodes.filter(node => Number.isFinite(Number(node.x)) && Number.isFinite(Number(node.y)));
    if (!positioned.length) {
      const empty = document.createElement('div');
      empty.className = 'canvas-status';
      empty.textContent = 'This canvas has no nodes.';
      host.append(empty);
      return;
    }
    const padding = 60;
    const minX = Math.min(...positioned.map(node => Number(node.x)));
    const minY = Math.min(...positioned.map(node => Number(node.y)));
    const maxX = Math.max(...positioned.map(node => Number(node.x) + Math.max(80, Number(node.width) || 260)));
    const maxY = Math.max(...positioned.map(node => Number(node.y) + Math.max(60, Number(node.height) || 140)));
    const boardWidth = Math.max(320, maxX - minX + padding * 2);
    const boardHeight = Math.max(260, maxY - minY + padding * 2);
    const left = value => Number(value) - minX + padding;
    const top = value => Number(value) - minY + padding;

    const toolbar = document.createElement('div');
    toolbar.className = 'canvas-toolbar';
    const hint = document.createElement('span');
    hint.className = 'canvas-hint';
    hint.textContent = 'Drag to pan · Ctrl + scroll to zoom';
    const zoomOut = document.createElement('button');
    zoomOut.type = 'button';
    zoomOut.className = 'icon-button';
    zoomOut.textContent = '−';
    zoomOut.title = 'Zoom out';
    const fit = document.createElement('button');
    fit.type = 'button';
    fit.className = 'icon-button';
    fit.textContent = 'Fit';
    fit.title = 'Fit canvas to view';
    const zoomIn = document.createElement('button');
    zoomIn.type = 'button';
    zoomIn.className = 'icon-button';
    zoomIn.textContent = '+';
    zoomIn.title = 'Zoom in';
    const zoomLabel = document.createElement('span');
    zoomLabel.textContent = '100%';
    toolbar.append(hint, zoomOut, fit, zoomIn, zoomLabel);

    const viewport = document.createElement('div');
    viewport.className = 'canvas-viewport';
    const stage = document.createElement('div');
    stage.className = 'canvas-stage';
    const board = document.createElement('div');
    board.className = 'canvas-board';
    board.style.width = boardWidth + 'px';
    board.style.height = boardHeight + 'px';
    stage.append(board);
    viewport.append(stage);
    host.append(toolbar, viewport);

    // Drag the board with the left mouse button. Interactive nodes keep their
    // normal click behaviour; dragging anywhere else pans the viewport.
    let panning = false;
    let panStartX = 0;
    let panStartY = 0;
    let panStartLeft = 0;
    let panStartTop = 0;
    viewport.addEventListener('pointerdown', event => {
      if (event.button !== 0 || event.pointerType === 'touch') return;
      if (event.target.closest && event.target.closest('a, button')) return;
      panning = true;
      panStartX = event.clientX;
      panStartY = event.clientY;
      panStartLeft = viewport.scrollLeft;
      panStartTop = viewport.scrollTop;
      viewport.classList.add('is-panning');
      if (viewport.setPointerCapture) viewport.setPointerCapture(event.pointerId);
      event.preventDefault();
    });
    viewport.addEventListener('pointermove', event => {
      if (!panning) return;
      viewport.scrollLeft = panStartLeft - (event.clientX - panStartX);
      viewport.scrollTop = panStartTop - (event.clientY - panStartY);
    });
    const endPan = event => {
      if (!panning) return;
      panning = false;
      viewport.classList.remove('is-panning');
      if (viewport.hasPointerCapture && viewport.hasPointerCapture(event.pointerId)) viewport.releasePointerCapture(event.pointerId);
    };
    viewport.addEventListener('pointerup', endPan);
    viewport.addEventListener('pointercancel', endPan);

    let scale = 1;
    const applyScale = next => {
      scale = clamp(next, 0.2, 2.5);
      board.style.transform = 'scale(' + scale + ')';
      stage.style.width = (boardWidth * scale) + 'px';
      stage.style.height = (boardHeight * scale) + 'px';
      zoomLabel.textContent = Math.round(scale * 100) + '%';
    };
    const fitToView = () => {
      const availableWidth = Math.max(120, viewport.clientWidth - 28);
      const availableHeight = Math.max(120, viewport.clientHeight - 28);
      applyScale(Math.min(availableWidth / boardWidth, availableHeight / boardHeight, 1));
    };
    zoomOut.addEventListener('click', () => applyScale(scale - 0.1));
    zoomIn.addEventListener('click', () => applyScale(scale + 0.1));
    fit.addEventListener('click', fitToView);
    viewport.addEventListener('wheel', event => {
      if (!event.ctrlKey) return;
      event.preventDefault();
      applyScale(scale + (event.deltaY < 0 ? 0.1 : -0.1));
    }, { passive: false });

    const edgeLayer = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
    edgeLayer.classList.add('canvas-edges');
    edgeLayer.setAttribute('viewBox', '0 0 ' + boardWidth + ' ' + boardHeight);
    const definitions = document.createElementNS('http://www.w3.org/2000/svg', 'defs');
    const marker = document.createElementNS('http://www.w3.org/2000/svg', 'marker');
    marker.setAttribute('id', 'canvas-arrow');
    marker.setAttribute('viewBox', '0 0 10 10');
    marker.setAttribute('refX', '9');
    marker.setAttribute('refY', '5');
    marker.setAttribute('markerWidth', '7');
    marker.setAttribute('markerHeight', '7');
    marker.setAttribute('orient', 'auto-start-reverse');
    const arrow = document.createElementNS('http://www.w3.org/2000/svg', 'path');
    arrow.setAttribute('d', 'M0 0 L10 5 L0 10 z');
    arrow.setAttribute('class', 'canvas-arrow-head');
    marker.append(arrow);
    definitions.append(marker);

    const groupNodes = positioned.filter(node => node.type === 'group');
    const contentNodes = positioned.filter(node => node.type !== 'group');
    const makeNode = node => {
      const element = document.createElement(node.type === 'file' || node.type === 'link' ? 'a' : 'div');
      element.className = 'canvas-node canvas-' + (node.type || 'text');
      if (node.color) element.dataset.color = String(node.color);
      element.style.left = left(node.x) + 'px';
      element.style.top = top(node.y) + 'px';
      element.style.width = Math.max(80, Number(node.width) || 260) + 'px';
      element.style.height = Math.max(60, Number(node.height) || 140) + 'px';
      if (node.type === 'file') {
        const file = typeof node.file === 'string' ? node.file : '';
        element.href = canvasHref(file);
        const icon = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
        icon.setAttribute('class', 'icon');
        icon.setAttribute('aria-hidden', 'true');
        const use = document.createElementNS('http://www.w3.org/2000/svg', 'use');
        use.setAttribute('href', '#' + canvasIconFor(file));
        icon.append(use);
        const label = document.createElement('span');
        label.className = 'canvas-file-label';
        label.textContent = file.split('/').pop() || file;
        element.append(icon, label);
      } else if (node.type === 'link') {
        const url = typeof node.url === 'string' ? node.url : '';
        element.href = url;
        element.target = '_blank';
        element.rel = 'noopener';
        const icon = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
        icon.setAttribute('class', 'icon');
        icon.setAttribute('aria-hidden', 'true');
        const use = document.createElementNS('http://www.w3.org/2000/svg', 'use');
        use.setAttribute('href', '#i-link');
        icon.append(use);
        const label = document.createElement('span');
        label.className = 'canvas-link-url';
        label.textContent = url;
        element.append(icon, label);
      } else if (node.type === 'group') {
        element.textContent = typeof node.label === 'string' ? node.label : '';
      } else {
        element.classList.add('canvas-text');
        renderCanvasMarkdown(element, typeof node.text === 'string' ? node.text : '');
      }
      return element;
    };
    groupNodes.forEach(node => board.append(makeNode(node)));
    board.append(edgeLayer);
    contentNodes.forEach(node => board.append(makeNode(node)));

    const byId = new Map(positioned.map(node => [String(node.id), node]));
    const anchors = (node, side) => {
      const x = left(node.x), y = top(node.y);
      const width = Math.max(80, Number(node.width) || 260);
      const height = Math.max(60, Number(node.height) || 140);
      if (side === 'top') return [x + width / 2, y];
      if (side === 'bottom') return [x + width / 2, y + height];
      if (side === 'left') return [x, y + height / 2];
      return [x + width, y + height / 2];
    };
    const direction = side => side === 'top' ? [0, -1] : side === 'bottom' ? [0, 1] : side === 'left' ? [-1, 0] : [1, 0];
    edges.forEach(edge => {
      const from = byId.get(String(edge.fromNode));
      const to = byId.get(String(edge.toNode));
      if (!from || !to) return;
      const fromSide = edge.fromSide || 'right';
      const toSide = edge.toSide || 'left';
      const [x1, y1] = anchors(from, fromSide);
      const [x2, y2] = anchors(to, toSide);
      const [dx1, dy1] = direction(fromSide);
      const [dx2, dy2] = direction(toSide);
      const bend = clamp(Math.hypot(x2 - x1, y2 - y1) * 0.4, 36, 160);
      const path = document.createElementNS('http://www.w3.org/2000/svg', 'path');
      path.setAttribute('d', 'M' + x1 + ' ' + y1 + ' C' + (x1 + dx1 * bend) + ' ' + (y1 + dy1 * bend) + ' ' +
        (x2 + dx2 * bend) + ' ' + (y2 + dy2 * bend) + ' ' + x2 + ' ' + y2);
      path.setAttribute('marker-end', 'url(#canvas-arrow)');
      edgeLayer.append(path);
      if (typeof edge.label === 'string' && edge.label.trim()) {
        const label = document.createElementNS('http://www.w3.org/2000/svg', 'text');
        label.setAttribute('class', 'canvas-edge-label');
        label.setAttribute('x', String((x1 + x2) / 2));
        label.setAttribute('y', String((y1 + y2) / 2 - 6));
        label.setAttribute('text-anchor', 'middle');
        label.textContent = edge.label;
        edgeLayer.append(label);
      }
    });
    edgeLayer.prepend(definitions);
    applyScale(1);
    window.requestAnimationFrame(fitToView);
  };
  const setupCanvas = () => {
    const host = document.getElementById('canvas-host');
    if (!host || host.dataset.canvasState) return;
    host.dataset.canvasState = 'loading';
    fetch(host.dataset.canvasUrl, { cache: 'no-store' })
      .then(response => {
        if (!response.ok) throw new Error('Unable to load canvas');
        return response.json();
      })
      .then(data => {
        if (!host.isConnected) return;
        host.dataset.canvasState = 'ready';
        renderCanvasBoard(host, data);
      })
      .catch(() => {
        if (!host.isConnected) return;
        host.dataset.canvasState = 'error';
        host.innerHTML = '<div class="canvas-status">Unable to load this canvas. The file may be invalid JSON.</div>';
      });
  };

  /* In-page navigation ----------------------------------------------------- */

  let navigationController = null;
  const navigateNote = async (url, pushState) => {
    const target = new URL(url, location.href);
    let path = target.pathname;
    if (path.startsWith('/file/') && path.toLowerCase().endsWith('.md')) path = path.replace(/^\/file\//, '/note/');
    const kind = path.startsWith('/note/') ? 'note' : path.startsWith('/image/') ? 'image' : path.startsWith('/base/') ? 'base' : path.startsWith('/canvas/') ? 'canvas' : null;
    if (!kind) return false;
    if (navigationController) navigationController.abort();
    navigationController = new AbortController();
    content.classList.add('is-loading');
    try {
      const encodedPath = path.replace(/^\/(note|image|base|canvas)\//, '');
      const response = await fetch('/fragment/' + kind + '/' + encodedPath, { signal: navigationController.signal, cache: 'default' });
      if (!response.ok) throw new Error('Unable to load note');
      content.innerHTML = await response.text();
      content.classList.remove('is-loading');
      if (pushState) history.pushState({}, '', path + target.search + target.hash);
      const title = content.querySelector('.note-title')?.textContent || 'Vault';
      document.title = title + ' · VaultLite';
      applyWrap('right', getWrap('right'));
      rightSidebar.sync();
      syncTocControls();
      setupHeadingSearch();
      setupCanvas();
      if (kind === 'note') hydrateBacklinks(encodedPath);
      syncActiveFile();
      if (target.hash) {
        requestAnimationFrame(() => document.getElementById(decodeURIComponent(target.hash.slice(1)))?.scrollIntoView());
      } else {
        window.scrollTo(0, 0);
      }
      return true;
    } catch (error) {
      content.classList.remove('is-loading');
      if (error.name !== 'AbortError') location.assign(path + target.search + target.hash);
      return false;
    }
  };
  document.addEventListener('click', event => {
    const anchor = event.target.closest && event.target.closest('a[href]');
    if (!anchor || event.defaultPrevented || event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey || anchor.target) return;
    try {
      const target = new URL(anchor.href, location.href);
      const localHost = target.hostname === '127.0.0.1' || target.hostname === 'localhost' || target.hostname === '::1';
      if (localHost && (target.pathname.startsWith('/note/') || target.pathname.startsWith('/image/') || target.pathname.startsWith('/base/') || target.pathname.startsWith('/canvas/') || (target.pathname.startsWith('/file/') && target.pathname.toLowerCase().endsWith('.md')))) {
        event.preventDefault();
        navigateNote(target.href, true);
      }
    } catch (_) { /* not a local URL */ }
  });
  tree.addEventListener('click', event => {
    if (mobileQuery.matches && event.target.closest && event.target.closest('a.file')) leftSidebar.set(true);
  });
  window.addEventListener('popstate', () => {
    if (location.pathname.startsWith('/note/') || location.pathname.startsWith('/image/') || location.pathname.startsWith('/base/') || location.pathname.startsWith('/canvas/')) navigateNote(location.href, false);
    else location.reload();
  });

  /* Commands and keyboard shortcuts ---------------------------------------- */

  const commands = [
    { id: 'open-command-palette', title: 'Open command palette', keywords: 'commands shortcuts', run: () => palette.open() },
    { id: 'open-search', title: 'Search files', keywords: 'find quick open vault', run: openSearch },
    { id: 'toggle-left-sidebar', title: 'Toggle left sidebar', keywords: 'navigator files tree', run: () => leftSidebar.toggle() },
    { id: 'toggle-right-sidebar', title: 'Toggle right sidebar', keywords: 'table of contents toc outline', run: () => rightSidebar.toggle() },
    { id: 'toggle-toc-level-one', title: 'Toggle level 1 headings only', keywords: 'table of contents toc outline collapse headings', run: () => setTocLevelOne(!tocLevelOne) },
    { id: 'toggle-theme', title: 'Toggle dark mode', keywords: 'theme light appearance', run: () => setTheme(root.dataset.theme === 'dark' ? 'light' : 'dark') },
    { id: 'collapse-folders', title: 'Collapse all folders', keywords: 'tree navigator', run: collapseFolders },
    { id: 'expand-folders', title: 'Expand all folders', keywords: 'tree navigator', run: expandFolders },
    { id: 'font-increase', title: 'Increase note text size', keywords: 'font zoom bigger', run: () => setNoteFontSize(noteFontSize + 1) },
    { id: 'font-decrease', title: 'Decrease note text size', keywords: 'font zoom smaller', run: () => setNoteFontSize(noteFontSize - 1) },
    { id: 'reset-font-size', title: 'Reset note text size', keywords: 'font zoom default', run: () => setNoteFontSize(16) },
    { id: 'toggle-left-wrap', title: 'Toggle file name wrapping', keywords: 'navigator tree', run: () => setWrap('left', !getWrap('left')) },
    { id: 'toggle-right-wrap', title: 'Toggle table of contents wrapping', keywords: 'toc outline', run: () => setWrap('right', !getWrap('right')) },
    { id: 'choose-palette', title: 'Choose color palette', keywords: 'theme colors appearance', run: () => setPaletteMenu(paletteMenu.hidden) },
    { id: 'go-home', title: 'Go to vault home', keywords: 'start index home', run: () => { if (location.pathname !== '/') location.assign('/'); } },
    { id: 'copy-note', title: 'Copy note', keywords: 'clipboard markdown source current', run: copyNote },
    { id: 'open-in-obsidian', title: 'Open note in Obsidian', keywords: 'vault uri app external edit', run: openInObsidian },
    { id: 'open-config-folder', title: 'Open VaultLite config folder', keywords: 'settings commands launcher configuration', run: openConfigFolder },
    { id: 'exit-app', title: 'Exit VaultLite and stop server', keywords: 'quit close shutdown terminate', run: exitApp }
  ];
  commands.forEach(command => {
    command.binding = BINDINGS[command.id] || '';
    command.parsed = parseBinding(command.binding);
    command.allowInInput = true;
  });
  const runCommand = id => {
    const command = commands.find(candidate => candidate.id === id);
    if (command) command.run();
    return !!command;
  };
  document.addEventListener('click', event => {
    const target = event.target.closest && event.target.closest('[data-command]');
    if (!target) return;
    event.preventDefault();
    runCommand(target.dataset.command);
  });

  function parseBinding(binding) {
    if (!binding) return null;
    const parts = binding.split('+').map(part => part.trim()).filter(Boolean);
    if (parts.length < 2) return null;
    const rawKey = parts.pop();
    if (!rawKey) return null;
    const modifiers = { ctrl: false, alt: false, shift: false, meta: false };
    for (const part of parts) {
      const name = part.toLowerCase();
      if (name === 'ctrl' || name === 'control') modifiers.ctrl = true;
      else if (name === 'alt' || name === 'option') modifiers.alt = true;
      else if (name === 'shift') modifiers.shift = true;
      else if (name === 'meta' || name === 'cmd' || name === 'command' || name === 'super') modifiers.meta = true;
      else return null;
    }
    return { key: normalizeKey(rawKey), modifiers };
  }
  function normalizeKey(key) {
    if (key === ' ') return 'Space';
    if (key === 'Esc') return 'Escape';
    if (key.length === 1) return key.toUpperCase();
    return key;
  }
  const bindingMatches = (binding, event) => {
    if (!binding) return false;
    const key = normalizeKey(event.key);
    if (key.toLocaleLowerCase() !== binding.key.toLocaleLowerCase()) return false;
    const modifiers = binding.modifiers;
    if (modifiers.alt !== event.altKey || modifiers.shift !== event.shiftKey) return false;
    // Ctrl and Cmd are interchangeable for Ctrl bindings on macOS, matching
    // Obsidian's cross-platform behaviour.
    if (modifiers.ctrl) {
      if (isMac ? !(event.ctrlKey || event.metaKey) : !event.ctrlKey) return false;
      return true;
    }
    return modifiers.meta === event.metaKey && !event.ctrlKey;
  };
  const formatShortcut = binding => {
    if (!binding) return '';
    if (!isMac) return binding.replace(/\bMeta\b/gi, 'Win').replace(/\bCommand\b/gi, 'Win');
    return binding
      .replace(/\bCtrl\b/gi, '⌘')
      .replace(/\bMeta\b/gi, '⌘')
      .replace(/\bCommand\b/gi, '⌘')
      .replace(/\bAlt\b/gi, '⌥')
      .replace(/\bOption\b/gi, '⌥')
      .replace(/\bShift\b/gi, '⇧');
  };
  document.querySelectorAll('[data-command="open-command-palette"]').forEach(button => {
    const shortcut = formatShortcut(BINDINGS['open-command-palette'] || '');
    button.title = shortcut ? 'Open command palette (' + shortcut + ')' : 'Open command palette';
    button.setAttribute('aria-label', button.title);
  });

  /* Command palette -------------------------------------------------------- */

  const paletteElement = document.getElementById('command-palette');
  const paletteInput = document.getElementById('command-input');
  const paletteList = document.getElementById('command-list');
  let paletteItems = [];
  let paletteIndex = 0;

  const renderPalette = query => {
    paletteItems = commands
      .map(command => ({ command, score: fuzzyScore(query, normalizeSearchText(command.title + ' ' + (command.keywords || '')), '') }))
      .filter(entry => entry.score !== -Infinity)
      .sort((a, b) => b.score - a.score)
      .map(entry => entry.command);
    paletteIndex = clamp(paletteIndex, 0, Math.max(0, paletteItems.length - 1));
    paletteList.replaceChildren();
    if (!paletteItems.length) {
      const empty = document.createElement('div');
      empty.className = 'command-empty';
      empty.textContent = 'No matching commands';
      paletteList.append(empty);
      return;
    }
    paletteItems.forEach((command, index) => {
      const option = document.createElement('button');
      option.type = 'button';
      option.className = 'command-option';
      option.dataset.commandId = command.id;
      option.setAttribute('role', 'option');
      option.setAttribute('aria-selected', String(index === paletteIndex));
      const label = document.createElement('span');
      label.className = 'command-label';
      label.textContent = command.title;
      option.append(label);
      const shortcut = formatShortcut(command.binding);
      if (shortcut) {
        const key = document.createElement('span');
        key.className = 'command-key';
        key.textContent = shortcut;
        option.append(key);
      }
      paletteList.append(option);
    });
  };
  const selectPaletteIndex = index => {
    paletteIndex = clamp(index, 0, Math.max(0, paletteItems.length - 1));
    paletteList.querySelectorAll('.command-option').forEach((option, optionIndex) => {
      option.setAttribute('aria-selected', String(optionIndex === paletteIndex));
    });
    const selected = paletteList.querySelector('.command-option[aria-selected="true"]');
    if (selected) selected.scrollIntoView({ block: 'nearest' });
  };
  const palette = {
    isOpen() { return !paletteElement.hidden; },
    open() {
      if (this.isOpen()) return;
      paletteInput.value = '';
      paletteIndex = 0;
      renderPalette('');
      paletteElement.hidden = false;
      body.classList.add('command-open');
      paletteInput.focus();
    },
    close() {
      paletteElement.hidden = true;
      body.classList.remove('command-open');
      paletteInput.blur();
    },
    run(command) {
      this.close();
      command.run();
    }
  };
  paletteInput.addEventListener('input', () => {
    paletteIndex = 0;
    renderPalette(paletteInput.value);
  });
  paletteInput.addEventListener('keydown', event => {
    if (event.key === 'Escape') {
      event.preventDefault();
      palette.close();
    } else if (event.key === 'ArrowDown') {
      event.preventDefault();
      selectPaletteIndex(paletteIndex + 1);
    } else if (event.key === 'ArrowUp') {
      event.preventDefault();
      selectPaletteIndex(paletteIndex - 1);
    } else if (event.key === 'Enter') {
      event.preventDefault();
      const command = paletteItems[paletteIndex];
      if (command) palette.run(command);
    }
  });
  paletteList.addEventListener('click', event => {
    const option = event.target.closest('.command-option');
    if (!option) return;
    const command = commands.find(candidate => candidate.id === option.dataset.commandId);
    if (command) palette.run(command);
  });
  paletteElement.addEventListener('mousedown', event => {
    if (event.target === paletteElement) palette.close();
  });

  /* Global shortcut dispatch ----------------------------------------------- */

  if (searchHint) {
    const shortcut = formatShortcut(BINDINGS['open-search'] || '');
    searchHint.textContent = shortcut;
    searchHint.hidden = !shortcut;
  }
  document.addEventListener('keydown', event => {
    if (event.defaultPrevented || event.isComposing) return;
    if (palette.isOpen()) return;
    if (event.key === 'Escape' && !paletteMenu.hidden) {
      setPaletteMenu(false);
      return;
    }
    if (event.key === '/' && !isEditing(event.target)) {
      event.preventDefault();
      openSearch();
      return;
    }
    for (const command of commands) {
      if (!command.parsed) continue;
      if (isEditing(event.target) && !command.allowInInput) continue;
      if (bindingMatches(command.parsed, event)) {
        event.preventDefault();
        runCommand(command.id);
        return;
      }
    }
  });

  /* Direct view controls --------------------------------------------------- */

  document.getElementById('sidebar-smaller').addEventListener('click', () => setSidebarWidth(sidebarWidth - 30));
  document.getElementById('sidebar-larger').addEventListener('click', () => setSidebarWidth(sidebarWidth + 30));
  document.getElementById('font-smaller').addEventListener('click', () => setNoteFontSize(noteFontSize - 1));
  document.getElementById('font-larger').addEventListener('click', () => setNoteFontSize(noteFontSize + 1));
  document.addEventListener('click', event => {
    const resize = event.target.closest && event.target.closest('[data-toc-resize]');
    if (resize) {
      setTocWidth(tocWidth + (resize.dataset.tocResize === 'larger' ? 30 : -30));
      return;
    }
    const level = event.target.closest && event.target.closest('[data-toc-level-one]');
    if (level) {
      setTocLevelOne(!tocLevelOne);
      return;
    }
    const control = event.target.closest && event.target.closest('[data-wrap-control]');
    if (!control) return;
    const side = control.dataset.wrapControl;
    setWrap(side, !getWrap(side));
  });
  mobileQuery.addEventListener('change', () => leftSidebar.sync());

  /* Initial view ----------------------------------------------------------- */

  syncActiveFile();
  setupCanvas();
  const initialNotePath = location.pathname.startsWith('/note/') ? location.pathname.slice('/note/'.length) :
    (location.pathname.startsWith('/file/') && location.pathname.toLowerCase().endsWith('.md') ? location.pathname.slice('/file/'.length) : null);
  if (initialNotePath) hydrateBacklinks(initialNotePath);
})();
