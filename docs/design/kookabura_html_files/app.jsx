// Kookaburra main App — strip + tile grid + interactions + tweaks

const TWEAKS = /*EDITMODE-BEGIN*/{
  "chaos": 2,
  "density": "normal",
  "accentHue": 68,
  "showLogoWalker": true,
  "showGrid": true,
  "helper": true,
  "helperChatty": true
}/*EDITMODE-END*/;

const LAYOUT_PRESETS = {
  '1x1': { cols: 1, rows: 1, count: 1 },
  '2x1': { cols: 2, rows: 1, count: 2 },
  '1x2': { cols: 1, rows: 2, count: 2 },
  '2x2': { cols: 2, rows: 2, count: 4 },
  '3x2': { cols: 3, rows: 2, count: 6 },
  '2x3': { cols: 2, rows: 3, count: 6 },
};

function App() {
  const t = window.THEME;
  const [wsIdx, setWsIdx] = React.useState(0);
  const [focused, setFocused] = React.useState(0);
  const [hovered, setHovered] = React.useState(null);
  const [zen, setZen] = React.useState(false);
  const [tweaks, setTweaks] = React.useState(TWEAKS);
  const [tweaksOpen, setTweaksOpen] = React.useState(true);
  const [squish, setSquish] = React.useState(null); // index to squish animate
  const [workspaces, setWorkspaces] = React.useState(window.WORKSPACES);
  const gridRef = React.useRef(null);
  const [tileRects, setTileRects] = React.useState([]);

  // tweak protocol
  React.useEffect(() => {
    const handler = (ev) => {
      if (ev.data?.type === '__activate_edit_mode') setTweaksOpen(true);
      if (ev.data?.type === '__deactivate_edit_mode') setTweaksOpen(false);
    };
    window.addEventListener('message', handler);
    window.parent.postMessage({ type: '__edit_mode_available' }, '*');
    return () => window.removeEventListener('message', handler);
  }, []);

  const updateTweak = (key, val) => {
    const next = { ...tweaks, [key]: val };
    setTweaks(next);
    window.parent.postMessage({ type: '__edit_mode_set_keys', edits: { [key]: val } }, '*');
  };

  // keyboard
  React.useEffect(() => {
    const onKey = (e) => {
      if (e.target.tagName === 'INPUT' || e.target.tagName === 'TEXTAREA') return;
      if ((e.metaKey || e.ctrlKey) && e.key >= '1' && e.key <= '9') {
        const idx = parseInt(e.key, 10) - 1;
        if (idx < workspaces.length) { switchWs(idx); e.preventDefault(); }
      } else if (e.key.toLowerCase() === 'z' && !e.metaKey) {
        setZen(z => !z);
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [workspaces]);

  const ws = workspaces[wsIdx];
  const preset = LAYOUT_PRESETS[ws.layout];
  const tiles = ws.tiles.slice(0, preset.count);
  const primary = tiles.findIndex(tl => tl.primary);

  const switchWs = (i) => {
    if (i === wsIdx) return;
    setWsIdx(i);
    setFocused(workspaces[i].tiles.findIndex(tl => tl.primary) >= 0 ? workspaces[i].tiles.findIndex(tl => tl.primary) : 0);
    setSquish(i);
    setTimeout(() => setSquish(null), 350);
  };

  const onTileAction = (kind, idx) => {
    if (kind === 'primary') {
      const next = workspaces.map((w, i) => i === wsIdx ? {
        ...w, tiles: w.tiles.map((tl, j) => ({ ...tl, primary: j === idx ? !tl.primary : false }))
      } : w);
      setWorkspaces(next);
    } else if (kind === 'follow') {
      const next = workspaces.map((w, i) => i === wsIdx ? {
        ...w, tiles: w.tiles.map((tl, j) => j === idx ? { ...tl, follow: !tl.follow } : tl)
      } : w);
      setWorkspaces(next);
    } else if (kind === 'close') {
      const next = workspaces.map((w, i) => i === wsIdx ? {
        ...w, tiles: w.tiles.filter((_, j) => j !== idx)
      } : w);
      setWorkspaces(next);
      if (focused >= next[wsIdx].tiles.length) setFocused(Math.max(0, next[wsIdx].tiles.length - 1));
    } else if (kind === 'fork') {
      const next = workspaces.map((w, i) => i === wsIdx ? {
        ...w, tiles: [...w.tiles.slice(0, idx + 1), {
          ...w.tiles[idx],
          branch: w.tiles[idx].branch?.replace(/-[a-f0-9]+$/, '-' + Math.random().toString(16).slice(2, 5)),
          primary: false, generating: true,
        }, ...w.tiles.slice(idx + 1)],
      } : w);
      setWorkspaces(next);
    }
  };

  // tile grid css
  const gridStyle = zen ? { gridTemplateColumns: '1fr', gridTemplateRows: '1fr' } : {
    gridTemplateColumns: `repeat(${preset.cols}, 1fr)`,
    gridTemplateRows: `repeat(${preset.rows}, 1fr)`,
  };

  const visibleTiles = zen ? [tiles[focused]] : tiles;

  // accent override
  const accentHue = tweaks.accentHue;
  const themeOverride = { '--koo-accent': `oklch(0.80 0.17 ${accentHue})` };

  return (
    <div className="koo-app" style={{
      width: 1384, height: 864,
      border: `2px solid ${t.gridLine}`,
      background: t.bg,
      display: 'flex', flexDirection: 'column',
      boxShadow: '0 20px 80px rgba(0,0,0,0.7), 0 0 0 1px oklch(0.24 0.012 60)',
      position: 'relative',
      overflow: 'hidden',
      ...themeOverride,
    }}>
      {/* window chrome: traffic lights + brand + global hotkeys */}
      <div style={{
        height: 30,
        background: t.bgDeep,
        borderBottom: `1px solid ${t.gridLine}`,
        display: 'flex', alignItems: 'center',
        padding: '0 12px',
        gap: 10,
        flexShrink: 0,
      }}>
        <TrafficLights />
        <div style={{ width: 20 }}/>
        <window.PixelLogo size={18} peck={tweaks.showLogoWalker} />
        <div className="koo-brand-type" style={{
          fontFamily: window.MONO, fontSize: 11, letterSpacing: 2,
          color: t.fgDim, textTransform: 'uppercase',
        }}>Kookaburra</div>
        <div style={{ flex: 1 }}/>
        <HotkeyHints zen={zen} />
      </div>

      {/* strip */}
      {!zen && <Strip
        workspaces={workspaces}
        wsIdx={wsIdx}
        onSwitch={switchWs}
        squish={squish}
        chaos={tweaks.chaos}
      />}

      {/* tile grid */}
      <div ref={gridRef} style={{
        flex: 1, display: 'grid', gap: 4,
        padding: 4,
        background: t.gridLine,
        ...gridStyle,
        minHeight: 0,
        position: 'relative',
      }}>
        {visibleTiles.map((tile, i) => (
          <window.Tile
            key={`${wsIdx}-${i}`}
            tile={tile}
            index={zen ? focused : i}
            focused={(zen ? focused : i) === focused}
            primary={primary === (zen ? focused : i)}
            onFocus={(ix) => setFocused(ix)}
            onAction={onTileAction}
            hovered={hovered}
            onHover={setHovered}
            dense={tweaks.density === 'dense'}
          />
        ))}
      </div>

      {/* footer status bar */}
      <StatusBar ws={ws} focused={focused} tiles={tiles} zen={zen} onZen={() => setZen(z => !z)} />

      {/* helper friend — Kooka */}
      <KookaHost
        gridRef={gridRef}
        workspaces={workspaces}
        wsIdx={wsIdx}
        zen={zen}
        focused={focused}
        tiles={visibleTiles}
        enabled={tweaks.helper}
        chatty={tweaks.helperChatty}
        chaos={tweaks.chaos}
      />

      {/* decorative walker bird */}
      {tweaks.showLogoWalker && !zen && <WalkerBird />}

      {/* tweaks panel */}
      {tweaksOpen && <TweaksPanel tweaks={tweaks} onChange={updateTweak} onClose={() => setTweaksOpen(false)} />}
    </div>
  );
}

function TrafficLights() {
  // pixel-style: not rounded, chunky squares
  return (
    <div className="koo-traffic" style={{ display: 'flex', gap: 5 }}>
      <div style={{ width: 10, height: 10, background: 'oklch(0.68 0.16 25)' }}/>
      <div style={{ width: 10, height: 10, background: 'oklch(0.82 0.14 90)' }}/>
      <div style={{ width: 10, height: 10, background: 'oklch(0.78 0.16 145)' }}/>
    </div>
  );
}

function HotkeyHints({ zen }) {
  const t = window.THEME;
  const hint = (k, l) => (
    <span style={{ display: 'inline-flex', gap: 4, alignItems: 'center' }}>
      <span style={{
        fontFamily: window.MONO, fontSize: 9.5,
        padding: '1px 4px',
        border: `1px solid ${t.gridLine}`,
        color: t.fgDim,
        background: t.bg,
      }}>{k}</span>
      <span style={{ fontFamily: window.MONO, fontSize: 10, color: t.fgFaint }}>{l}</span>
    </span>
  );
  return (
    <div style={{ display: 'flex', gap: 10, alignItems: 'center' }}>
      {hint('⌘1-9', 'ws')}
      {hint('⌘T', 'new tile')}
      {hint('⌘⇧T', 'new ws')}
      {hint('Z', zen ? 'exit zen' : 'zen')}
      {hint('⌘F', 'find')}
    </div>
  );
}

// ─── Strip ──────────────────────────────────────────────────────
function Strip({ workspaces, wsIdx, onSwitch, squish, chaos }) {
  const t = window.THEME;
  return (
    <div className="koo-strip" style={{
      height: 76,
      background: t.bg,
      borderBottom: `1px solid ${t.gridLine}`,
      padding: '10px 14px',
      display: 'flex', alignItems: 'center',
      gap: 10,
      flexShrink: 0,
      position: 'relative',
    }}>
      {/* brand anchor */}
      <div style={{
        display: 'flex', alignItems: 'center', gap: 6, paddingRight: 8,
        borderRight: `1px solid ${t.gridLine}`, marginRight: 4, height: 52,
      }}>
        <window.PixelLogo size={28} />
      </div>

      <div className="koo-strip-scroll" style={{
        display: 'flex', gap: 10, overflowX: 'auto',
        paddingBottom: 2, paddingTop: 2,
        flex: 1,
      }}>
        {workspaces.map((ws, i) => (
          <div
            key={ws.id}
            style={{
              animation: squish === i ? 'koo-ws-squish 340ms cubic-bezier(.2,.9,.3,1.3)' : 'none',
              transformOrigin: 'bottom center',
            }}
          >
            <window.WorkspaceCard
              ws={ws}
              index={i}
              active={i === wsIdx}
              onClick={onSwitch}
              onRename={() => {}}
              chaos={chaos}
            />
          </div>
        ))}

        <NewWorkspaceBtn />
      </div>

      {/* search */}
      <div style={{
        display: 'flex', alignItems: 'center', gap: 6,
        padding: '6px 10px',
        background: t.bgDeep,
        border: `1px solid ${t.gridLine}`,
        color: t.fgFaint,
        fontFamily: window.MONO, fontSize: 11,
        height: 32,
        minWidth: 180,
      }}>
        <span>⌕</span>
        <span style={{ letterSpacing: 0 }}>search all tiles…</span>
        <div style={{ flex: 1 }}/>
        <span style={{
          padding: '0 4px',
          border: `1px solid ${t.gridLine}`,
          fontSize: 9,
        }}>⌘⇧F</span>
      </div>
    </div>
  );
}

function NewWorkspaceBtn() {
  const t = window.THEME;
  const [hov, setHov] = React.useState(false);
  return (
    <button
      onMouseEnter={() => setHov(true)}
      onMouseLeave={() => setHov(false)}
      style={{
        width: 52, height: 52,
        background: t.bgDeep,
        border: `2px dashed ${hov ? t.accent : t.gridLine}`,
        color: hov ? t.accent : t.fgFaint,
        fontFamily: window.MONO, fontSize: 18,
        cursor: 'pointer',
        flexShrink: 0,
        transition: 'border-color 100ms, color 100ms',
      }}
    >+</button>
  );
}

// ─── Status bar ─────────────────────────────────────────────────
function StatusBar({ ws, focused, tiles, zen, onZen }) {
  const t = window.THEME;
  const tile = tiles[focused];
  return (
    <div style={{
      height: 22,
      background: t.bgDeep,
      borderTop: `1px solid ${t.gridLine}`,
      display: 'flex', alignItems: 'center',
      padding: '0 10px',
      gap: 12,
      fontFamily: window.MONO, fontSize: 10,
      color: t.fgDim,
      flexShrink: 0,
    }}>
      <span style={{ color: t.accent }}>●</span>
      <span>{ws.label}</span>
      <Sep />
      <span>tile {focused + 1}/{tiles.length}</span>
      <Sep />
      <span>{tile?.cwd || '~'}</span>
      {tile?.branch && <>
        <Sep />
        <span style={{ color: tile.worktree ? t.teal : t.fgDim }}>
          {tile.worktree ? 'wt ' : '⎇ '}{tile.branch}
        </span>
      </>}
      <div style={{ flex: 1 }}/>
      <span>{tiles.filter(tl => tl.generating).length} generating</span>
      <Sep />
      <span>120×32</span>
      <Sep />
      <button onClick={onZen} style={{
        background: zen ? t.accent : 'transparent',
        color: zen ? t.bgDeep : t.fgDim,
        border: 'none', padding: '0 6px', height: 16,
        fontFamily: window.MONO, fontSize: 10,
        cursor: 'pointer',
      }}>{zen ? 'ZEN ▣' : 'zen □'}</button>
      <Sep />
      <span style={{ color: t.green }}>● ready</span>
    </div>
  );
}

const Sep = () => <span style={{ color: window.THEME.gridLine }}>│</span>;

// ─── Walker bird (decorative, hops across the strip) ────────────
function WalkerBird() {
  const [x, setX] = React.useState(0);
  React.useEffect(() => {
    const t = setInterval(() => {
      setX(prev => (prev + 32) % 1200);
    }, 2000);
    return () => clearInterval(t);
  }, []);
  return (
    <div style={{
      position: 'absolute',
      top: 100, left: 40 + x,
      pointerEvents: 'none',
      transition: 'left 1.5s cubic-bezier(.3,1.3,.5,1)',
      zIndex: 20,
      opacity: 0.35,
    }}>
      <window.PixelLogo size={14} peck />
    </div>
  );
}

// ─── Tweaks panel ───────────────────────────────────────────────
function TweaksPanel({ tweaks, onChange, onClose }) {
  const t = window.THEME;
  return (
    <div style={{
      position: 'absolute',
      right: 16, bottom: 36,
      width: 240,
      background: t.bgDeep,
      border: `2px solid ${t.accent}`,
      fontFamily: window.MONO, fontSize: 11,
      color: t.fg,
      zIndex: 50,
      boxShadow: '0 8px 24px rgba(0,0,0,0.6)',
    }}>
      <div style={{
        display: 'flex', alignItems: 'center',
        padding: '6px 8px',
        background: t.accent, color: t.bgDeep,
        fontWeight: 600, letterSpacing: 1, textTransform: 'uppercase', fontSize: 10,
      }}>
        <window.PixelLogo size={12} />
        <span style={{ marginLeft: 6, flex: 1 }}>Tweaks</span>
        <button onClick={onClose} style={{
          background: 'transparent', border: 'none', color: t.bgDeep,
          fontFamily: window.MONO, fontWeight: 700, cursor: 'pointer', fontSize: 12,
          padding: 0, lineHeight: 1,
        }}>×</button>
      </div>

      <div style={{ padding: 10, display: 'flex', flexDirection: 'column', gap: 10 }}>
        <TweakRow label="chaos">
          <input
            type="range" min={0} max={4} step={1}
            value={tweaks.chaos}
            onChange={(e) => onChange('chaos', parseInt(e.target.value, 10))}
            style={{ width: '100%', accentColor: t.accent }}
          />
          <span style={{ color: t.fgDim, fontSize: 9.5 }}>
            {['none', 'subtle', 'lively', 'wacky', 'unhinged'][tweaks.chaos]}
          </span>
        </TweakRow>

        <TweakRow label="density">
          <div style={{ display: 'flex', gap: 4 }}>
            {['normal', 'dense'].map(d => (
              <button key={d} onClick={() => onChange('density', d)} style={{
                flex: 1, padding: '3px 6px',
                background: tweaks.density === d ? t.accent : 'transparent',
                color: tweaks.density === d ? t.bgDeep : t.fgDim,
                border: `1px solid ${tweaks.density === d ? t.accent : t.gridLine}`,
                fontFamily: window.MONO, fontSize: 10, cursor: 'pointer',
              }}>{d}</button>
            ))}
          </div>
        </TweakRow>

        <TweakRow label={`accent hue ${tweaks.accentHue}°`}>
          <input
            type="range" min={0} max={360} step={4}
            value={tweaks.accentHue}
            onChange={(e) => onChange('accentHue', parseInt(e.target.value, 10))}
            style={{ width: '100%', accentColor: `oklch(0.80 0.17 ${tweaks.accentHue})` }}
          />
        </TweakRow>

        <TweakRow label="helper friend (kooka)">
          <Toggle value={tweaks.helper} onChange={(v) => onChange('helper', v)} />
        </TweakRow>

        <TweakRow label="helper chatty">
          <Toggle value={tweaks.helperChatty} onChange={(v) => onChange('helperChatty', v)} />
        </TweakRow>

        <TweakRow label="show walker bird">
          <Toggle value={tweaks.showLogoWalker} onChange={(v) => onChange('showLogoWalker', v)} />
        </TweakRow>
      </div>
    </div>
  );
}

function TweakRow({ label, children }) {
  const t = window.THEME;
  return (
    <div>
      <div style={{
        fontSize: 9.5, letterSpacing: 1, textTransform: 'uppercase',
        color: t.fgFaint, marginBottom: 3,
      }}>{label}</div>
      {children}
    </div>
  );
}

function Toggle({ value, onChange }) {
  const t = window.THEME;
  return (
    <button onClick={() => onChange(!value)} style={{
      width: 36, height: 16,
      background: value ? t.accent : t.gridLine,
      border: 'none',
      position: 'relative', cursor: 'pointer',
      padding: 0,
    }}>
      <div style={{
        position: 'absolute', top: 2,
        left: value ? 20 : 2,
        width: 12, height: 12,
        background: value ? t.bgDeep : t.fgDim,
        transition: 'left 120ms',
      }}/>
    </button>
  );
}

// ─── Mount ──────────────────────────────────────────────────────
ReactDOM.createRoot(document.getElementById('app')).render(<App />);
