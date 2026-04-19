// Kookaburra tile + strip components — chunky-pixel aesthetic.
// Everything steps on an 8px grid. Borders are 2px pixel-style.

const CELL_W = 7.8;   // monospace cell width approx
const CELL_H = 16;    // line height
const MONO = '"JetBrains Mono", "IBM Plex Mono", ui-monospace, Menlo, monospace';
const PIXEL_GRID = 8;

// ─── Pixel logo (from the SVG, drawn as divs) ───────────────────
// We parse the shipped SVG once on load; for inline we just embed an <img>.
function PixelLogo({ size = 24, walk = false, peck = false, color = 'white' }) {
  // CSS pixel-rendering preserves the 8px grid on scale.
  const wobble = walk ? 'koo-walk 0.7s steps(2) infinite' : 'none';
  const head = peck ? 'koo-peck 1.6s ease-in-out infinite' : 'none';
  return (
    <div style={{
      width: size, height: size, position: 'relative',
      animation: wobble,
    }}>
      <img
        src="assets/kookaburra.svg"
        alt=""
        style={{
          width: '100%', height: '100%',
          imageRendering: 'pixelated',
          filter: color === 'white' ? 'none' : `brightness(0) saturate(100%) invert(67%) sepia(72%) saturate(486%) hue-rotate(352deg) brightness(101%) contrast(97%)`,
          transformOrigin: '40% 70%',
          animation: head,
          display: 'block',
        }}
      />
    </div>
  );
}

// ─── Terminal cell content ──────────────────────────────────────
function TerminalBody({ tile, focused, scale = 1 }) {
  const color = (key) => window.ANSI[key] || window.ANSI.d;
  return (
    <div className="koo-terminal-body" style={{
      fontFamily: MONO,
      fontSize: 11.5 * scale,
      lineHeight: `${CELL_H * scale}px`,
      color: window.THEME.fg,
      padding: 10,
      flex: 1,
      overflow: 'hidden',
      position: 'relative',
      letterSpacing: 0,
      whiteSpace: 'pre',
      opacity: focused ? 1 : 0.82,
      filter: focused ? 'none' : 'saturate(0.85)',
    }}>
      {tile.lines.map((line, i) => (
        <div key={i} style={{ minHeight: CELL_H * scale }}>
          {line.map((span, j) => (
            <span key={j} style={{
              color: color(span.c),
              fontWeight: span.c === 'a' || span.c === 'A' ? 600 : 400,
            }}>{span.t || ' '}</span>
          ))}
        </div>
      ))}
    </div>
  );
}

// ─── One tile ───────────────────────────────────────────────────
function Tile({ tile, index, focused, primary, onFocus, onAction, hovered, onHover, dense }) {
  const t = window.THEME;
  const borderColor = focused ? t.accent : (primary ? t.accentDeep : t.gridLine);
  const borderWidth = focused ? 2 : 2;

  // Tile glitch-seam animation (pixel marching)
  const seamAnim = focused ? 'koo-seam 2.4s linear infinite' : 'none';

  return (
    <div
      className="koo-tile"
      onMouseEnter={() => onHover(index)}
      onMouseLeave={() => onHover(null)}
      onClick={() => onFocus(index)}
      style={{
        position: 'relative',
        background: t.bgDeep,
        border: `${borderWidth}px solid ${borderColor}`,
        boxShadow: focused ? `0 0 0 1px ${t.bgDeep}, inset 0 0 0 1px ${t.bg}` : 'none',
        display: 'flex',
        flexDirection: 'column',
        minHeight: 0,
        cursor: focused ? 'text' : 'pointer',
        overflow: 'hidden',
        transition: 'border-color 80ms linear',
      }}
    >
      {/* new-output pixel drip */}
      {tile.generating && <OutputDrip />}

      {/* marching seam on focused */}
      {focused && (
        <div style={{
          position: 'absolute', inset: -2, pointerEvents: 'none',
          background: `repeating-linear-gradient(90deg, ${t.accent} 0 8px, transparent 8px 16px)`,
          maskImage: 'linear-gradient(#000,#000), linear-gradient(#000,#000), linear-gradient(#000,#000), linear-gradient(#000,#000)',
          maskComposite: 'add',
          // simulated marching ants: use a small top bar only
          height: 2,
          top: -2,
          animation: seamAnim,
        }}/>
      )}

      {/* title bar */}
      <TileHeader
        tile={tile}
        focused={focused}
        primary={primary}
        hovered={hovered === index}
        onAction={onAction}
        index={index}
      />

      {/* body */}
      <TerminalBody tile={tile} focused={focused} scale={dense ? 0.92 : 1} />

      {/* follow-mode hint at bottom */}
      {tile.follow && (
        <div style={{
          position: 'absolute', right: 8, bottom: 6,
          fontFamily: MONO, fontSize: 9.5,
          color: t.teal,
          letterSpacing: 0.5,
          textTransform: 'uppercase',
          opacity: 0.9,
        }}>▼ follow</div>
      )}
    </div>
  );
}

function TileHeader({ tile, focused, primary, hovered, onAction, index }) {
  const t = window.THEME;
  const runningColor = tile.running === 'claude' ? t.accent
    : tile.running === 'nvim' ? t.green
    : tile.running === 'htop' ? t.magenta
    : tile.running === 'pnpm' ? t.blue
    : t.fgDim;

  return (
    <div className="koo-tile-header" style={{
      display: 'flex', alignItems: 'center',
      height: 22, padding: '0 8px',
      background: focused ? t.bgDim : t.bg,
      borderBottom: `1px solid ${t.gridLine}`,
      gap: 8,
      fontFamily: MONO,
      fontSize: 10.5,
      flexShrink: 0,
      position: 'relative',
    }}>
      {/* tile index chip */}
      <div style={{
        width: 16, height: 14,
        display: 'flex', alignItems: 'center', justifyContent: 'center',
        background: focused ? t.accent : t.gridLine,
        color: focused ? t.bgDeep : t.fgDim,
        fontWeight: 600,
        letterSpacing: 0,
      }}>{index + 1}</div>

      {/* running process chip */}
      <div className="koo-tile-run" style={{
        display: 'flex', alignItems: 'center', gap: 4,
        color: runningColor,
      }}>
        <span style={{
          width: 6, height: 6, background: runningColor,
          display: 'inline-block',
          animation: tile.generating ? 'koo-pulse 1.2s ease-in-out infinite' : 'none',
        }}/>
        <span>{tile.running}</span>
      </div>

      {/* title */}
      <div className="koo-tile-title" style={{
        color: focused ? t.fg : t.fgDim,
        overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap',
        flex: 1,
      }}>
        {tile.title}
      </div>

      {/* branch chip */}
      {tile.branch && (
        <div style={{
          display: 'flex', alignItems: 'center', gap: 4,
          color: tile.worktree ? t.teal : t.fgDim,
          border: `1px solid ${tile.worktree ? t.teal : t.gridLine}`,
          padding: '0 4px', height: 14,
          fontSize: 9.5,
        }}>
          <span style={{ opacity: 0.7 }}>{tile.worktree ? 'wt' : '⎇'}</span>
          <span>{tile.branch}</span>
        </div>
      )}

      {/* primary star */}
      {primary && (
        <div title="primary tile" style={{
          color: t.accent, fontSize: 11, lineHeight: 1,
          width: 14, textAlign: 'center',
        }}>◆</div>
      )}

      {/* hover controls */}
      {hovered && (
        <div style={{ display: 'flex', gap: 2 }}>
          <TileBtn label={primary ? '◆' : '◇'} tip="primary" onClick={(e) => { e.stopPropagation(); onAction('primary', index); }} />
          <TileBtn label={tile.follow ? '▼' : '▽'} tip="follow" onClick={(e) => { e.stopPropagation(); onAction('follow', index); }} />
          {tile.worktree && <TileBtn label="⑂" tip="fork" onClick={(e) => { e.stopPropagation(); onAction('fork', index); }} />}
          <TileBtn label="×" tip="close" onClick={(e) => { e.stopPropagation(); onAction('close', index); }} danger />
        </div>
      )}
    </div>
  );
}

function TileBtn({ label, tip, onClick, danger }) {
  const t = window.THEME;
  const [hov, setHov] = React.useState(false);
  return (
    <button
      onClick={onClick}
      onMouseEnter={() => setHov(true)}
      onMouseLeave={() => setHov(false)}
      title={tip}
      style={{
        width: 16, height: 14, padding: 0,
        background: hov ? (danger ? t.red : t.accent) : 'transparent',
        color: hov ? t.bgDeep : t.fgDim,
        border: 'none',
        fontFamily: MONO, fontSize: 11, lineHeight: '14px',
        cursor: 'pointer',
      }}
    >{label}</button>
  );
}

function OutputDrip() {
  const t = window.THEME;
  return (
    <div style={{
      position: 'absolute', right: -1, top: 22,
      width: 2, height: 40,
      background: t.accent,
      animation: 'koo-drip 2s ease-in-out infinite',
      pointerEvents: 'none',
      zIndex: 2,
    }}/>
  );
}

// ─── Workspace card (in the strip) ──────────────────────────────
function WorkspaceCard({ ws, index, active, onClick, onRename, chaos }) {
  const t = window.THEME;
  const generatingCount = ws.tiles.filter(tl => tl.generating).length;

  // squish animation target on active
  const scale = active ? 1 : 0.96;
  const lift = active ? -2 : 0;

  return (
    <button
      className="koo-card"
      onClick={() => onClick(index)}
      onDoubleClick={() => onRename(index)}
      style={{
        width: 176, height: 52,
        background: active ? t.bgDim : t.bgDeep,
        border: `2px solid ${active ? t.accent : t.gridLine}`,
        padding: '6px 10px 7px',
        display: 'flex', flexDirection: 'column',
        gap: 3,
        fontFamily: MONO, fontSize: 11,
        color: active ? t.fg : t.fgDim,
        cursor: 'pointer',
        position: 'relative',
        transform: `translateY(${lift}px) scale(${scale})`,
        transformOrigin: 'bottom center',
        transition: 'transform 140ms cubic-bezier(.2,.9,.2,1.2), border-color 120ms, background 120ms',
        flexShrink: 0,
        textAlign: 'left',
      }}
    >
      {/* Cmd+N hotkey chip */}
      <div style={{
        position: 'absolute', top: -2, right: -2,
        background: active ? t.accent : t.bg,
        color: active ? t.bgDeep : t.fgFaint,
        fontFamily: MONO, fontSize: 9,
        padding: '1px 3px',
        border: `1px solid ${active ? t.accent : t.gridLine}`,
      }}>⌘{index + 1}</div>

      {/* label row */}
      <div style={{
        display: 'flex', alignItems: 'center', gap: 6,
        fontWeight: active ? 600 : 400,
        letterSpacing: active ? 0 : 0,
        overflow: 'hidden',
      }}>
        {generatingCount > 0 && (
          <span style={{
            width: 6, height: 6, flexShrink: 0,
            background: t.accent,
            animation: 'koo-pulse 1.2s ease-in-out infinite',
          }}/>
        )}
        <span style={{ overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
          {ws.label}
        </span>
      </div>

      {/* tile indicators row */}
      <div style={{ display: 'flex', gap: 3, alignItems: 'center', marginTop: 1 }}>
        {ws.tiles.map((tl, i) => (
          <TileDot key={i} tile={tl} active={active} chaos={chaos} idx={i} />
        ))}
        <div style={{ flex: 1 }} />
        <span style={{
          fontSize: 9, color: active ? t.fgDim : t.fgFaint,
          letterSpacing: 0,
        }}>
          {ws.repo ? `~/${ws.repo}` : 'no repo'}
        </span>
      </div>

      {/* layout chip */}
      <div style={{
        position: 'absolute', bottom: -2, left: -2,
        background: t.bg,
        color: t.fgFaint,
        fontFamily: MONO, fontSize: 8.5,
        padding: '1px 3px',
        border: `1px solid ${t.gridLine}`,
      }}>{ws.layout}</div>
    </button>
  );
}

function TileDot({ tile, active, chaos, idx }) {
  const t = window.THEME;
  const color = tile.generating ? t.accent
    : tile.running === 'claude' ? t.accent
    : tile.running === 'nvim' ? t.green
    : tile.running === 'htop' ? t.magenta
    : tile.running === 'pnpm' ? t.blue
    : t.fgFaint;

  // tiny bounce based on chaos level
  const bounceDelay = (idx * 180) % 1000;
  return (
    <div className="koo-tile-dot" style={{
      width: 8, height: 8,
      background: color,
      opacity: active ? 1 : 0.5,
      animation: tile.generating ? 'koo-bounce 1.1s ease-in-out infinite' : 'none',
      animationDelay: `${bounceDelay}ms`,
    }}/>
  );
}

Object.assign(window, {
  PixelLogo, TerminalBody, Tile, WorkspaceCard, MONO,
  CELL_W, CELL_H, PIXEL_GRID,
});
