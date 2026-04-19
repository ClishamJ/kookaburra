// Kooka — the helper friend. A pixel kookaburra that hops between tile
// perches, reacts to activity, and occasionally says something.
//
// Lives as an overlay above the tile grid. Consumes:
//   - tileRects: [{left, top, right, bottom, id}] of currently visible tiles
//   - focusedIdx
//   - generatingIdxs
//   - wsIdx (triggers a "big hop" when it changes)

const KOOKA_SIZE = 32; // px
const PERCH_OFFSET = -6; // sit above the header line

const TIPS = [
  "⌘⇧T for a new workspace",
  "drag a tile onto a card to move it",
  "⌘Enter = zen mode",
  "press Z to zen real quick",
  "try ⌘⇧F to search every tile at once",
  "double-click a card label to rename",
  "worktree tiles have a ⑂ button — try it",
  "a tile marked ◆ wakes up first",
  "follow mode: tail the bottom line forever",
  "HDAHAHA!",
  "…",
  "watching claude work so you don't have to",
];

const MOODS = {
  idle:     { phrase: null,            wiggle: 0.3 },
  watching: { phrase: "watching…",     wiggle: 0.6 },
  cheering: { phrase: "tests pass!",   wiggle: 2.0 },
  hop:      { phrase: null,            wiggle: 0.0 },
  laugh:    { phrase: "HDAHAHA!",      wiggle: 1.5 },
  tip:      { phrase: null,            wiggle: 0.4 },
};

function Kooka({ tileRects, focused, generatingIdxs, wsIdx, enabled, chaos, chatty }) {
  const [pos, setPos] = React.useState({ x: 100, y: 100, dir: 1 });
  const [mood, setMood] = React.useState('idle');
  const [phrase, setPhrase] = React.useState(null);
  const [phraseKey, setPhraseKey] = React.useState(0);
  const [isHopping, setIsHopping] = React.useState(false);
  const [clickCount, setClickCount] = React.useState(0);
  const targetRef = React.useRef(null);

  // pick a target perch. Rules:
  //  - if any generating tile exists, pick a random one 50% of the time
  //  - else prefer focused tile 40% of the time
  //  - else random
  const pickTarget = React.useCallback(() => {
    if (!tileRects || tileRects.length === 0) return null;
    const r = Math.random();
    let idx;
    if (generatingIdxs.length > 0 && r < 0.5) {
      idx = generatingIdxs[Math.floor(Math.random() * generatingIdxs.length)];
    } else if (r < 0.75 && focused != null && tileRects[focused]) {
      idx = focused;
    } else {
      idx = Math.floor(Math.random() * tileRects.length);
    }
    const rect = tileRects[idx];
    if (!rect) return null;
    // pick a random x along the top of the tile, not too close to edges
    const margin = 20;
    const w = rect.right - rect.left;
    const x = rect.left + margin + Math.random() * Math.max(0, w - margin * 2 - KOOKA_SIZE);
    const y = rect.top + PERCH_OFFSET - KOOKA_SIZE;
    return { x, y, tileIdx: idx };
  }, [tileRects, focused, generatingIdxs]);

  // hopping loop
  React.useEffect(() => {
    if (!enabled) return;
    let timer;
    const schedule = () => {
      // interval scales inversely with chaos (0..4)
      const base = 4200 - chaos * 700;
      const delay = base + Math.random() * 1500;
      timer = setTimeout(() => {
        const target = pickTarget();
        if (!target) return schedule();
        hopTo(target);
        schedule();
      }, delay);
    };
    schedule();
    return () => clearTimeout(timer);
  }, [enabled, chaos, pickTarget]);

  // trigger big hop on workspace switch
  const lastWs = React.useRef(wsIdx);
  React.useEffect(() => {
    if (lastWs.current !== wsIdx) {
      lastWs.current = wsIdx;
      const target = pickTarget();
      if (target) {
        setPhrase("new workspace!");
        setPhraseKey(k => k + 1);
        setTimeout(() => setPhrase(null), 1400);
        hopTo(target, true);
      }
    }
  }, [wsIdx, pickTarget]);

  // react to generating changes
  const prevGenCount = React.useRef(0);
  React.useEffect(() => {
    const count = generatingIdxs.length;
    if (count > prevGenCount.current) {
      setMood('watching');
    } else if (count === 0 && prevGenCount.current > 0) {
      setMood('cheering');
      setPhrase("all done!");
      setPhraseKey(k => k + 1);
      setTimeout(() => { setPhrase(null); setMood('idle'); }, 1800);
    }
    prevGenCount.current = count;
  }, [generatingIdxs]);

  const hopTo = (target, big = false) => {
    setIsHopping(true);
    setPos(prev => ({
      x: target.x,
      y: target.y,
      dir: target.x > prev.x ? 1 : -1,
    }));
    targetRef.current = target;
    setTimeout(() => setIsHopping(false), big ? 900 : 650);
  };

  // periodic chatter
  React.useEffect(() => {
    if (!enabled || !chatty) return;
    const t = setInterval(() => {
      if (Math.random() < 0.3 + chaos * 0.1) {
        const tip = TIPS[Math.floor(Math.random() * TIPS.length)];
        setPhrase(tip);
        setPhraseKey(k => k + 1);
        setTimeout(() => setPhrase(null), 2400);
      }
    }, 6000);
    return () => clearInterval(t);
  }, [enabled, chatty, chaos]);

  const onClick = () => {
    const next = clickCount + 1;
    setClickCount(next);
    // cycle through phrases; every 5th click triggers the laugh
    if (next % 5 === 0) {
      setPhrase("HDAHAHAHAHA!");
      setMood('laugh');
      setTimeout(() => setMood('idle'), 2000);
    } else {
      setPhrase(TIPS[next % TIPS.length]);
    }
    setPhraseKey(k => k + 1);
    setTimeout(() => setPhrase(null), 2200);
  };

  if (!enabled || !tileRects || tileRects.length === 0) return null;

  const t = window.THEME;
  const hopTransform = isHopping ? 'koo-arc 650ms ease-out' : 'none';

  return (
    <>
      {/* shadow under bird */}
      <div className="koo-kooka-shadow" style={{
        position: 'absolute',
        left: pos.x + KOOKA_SIZE / 2 - 8,
        top: pos.y + KOOKA_SIZE + 4,
        width: 16, height: 3,
        background: 'oklch(0.05 0 0 / 0.5)',
        transition: 'left 650ms cubic-bezier(.3,.0,.2,1), top 650ms cubic-bezier(.3,.0,.2,1)',
        pointerEvents: 'none',
        zIndex: 40,
      }}/>

      {/* bird */}
      <div
        className="koo-kooka-sprite"
        onClick={onClick}
        style={{
          position: 'absolute',
          left: pos.x,
          top: pos.y,
          width: KOOKA_SIZE, height: KOOKA_SIZE,
          zIndex: 41,
          cursor: 'pointer',
          transition: isHopping
            ? 'left 650ms cubic-bezier(.35,-.2,.2,1), top 650ms cubic-bezier(.35,-.2,.2,1)'
            : 'left 650ms cubic-bezier(.35,-.2,.2,1), top 650ms cubic-bezier(.35,-.2,.2,1)',
          transform: `scaleX(${pos.dir}) ${isHopping ? 'translateY(-6px)' : ''}`,
          transformOrigin: 'center bottom',
          filter: 'drop-shadow(0 0 2px oklch(0.78 0.17 65 / 0.4))',
          animation: isHopping ? 'none' : `koo-idle-bob ${2.2 - chaos * 0.3}s ease-in-out infinite`,
        }}
      >
        <KookaSprite mood={mood} wiggleAmt={MOODS[mood].wiggle} chaos={chaos} generating={generatingIdxs.length > 0} />
      </div>

      {/* speech bubble */}
      {phrase && (
        <div
          className="koo-bubble"
          key={phraseKey}
          style={{
            position: 'absolute',
            left: pos.x + (pos.dir > 0 ? KOOKA_SIZE + 6 : -170),
            top: pos.y - 22,
            minWidth: 120, maxWidth: 220,
            background: t.bgDeep,
            border: `2px solid ${t.accent}`,
            color: t.fg,
            fontFamily: window.MONO, fontSize: 10.5,
            padding: '4px 8px',
            zIndex: 42,
            pointerEvents: 'none',
            transition: 'left 650ms cubic-bezier(.35,-.2,.2,1), top 650ms cubic-bezier(.35,-.2,.2,1)',
            animation: 'koo-bubble-in 180ms ease-out',
            letterSpacing: 0,
            whiteSpace: 'nowrap',
            overflow: 'hidden',
            textOverflow: 'ellipsis',
          }}
        >
          {phrase}
          {/* pixel tail */}
          <div style={{
            position: 'absolute',
            left: pos.dir > 0 ? -6 : 'auto',
            right: pos.dir > 0 ? 'auto' : -6,
            bottom: 4,
            width: 4, height: 4,
            background: t.accent,
          }}/>
        </div>
      )}
    </>
  );
}

// Tiny sprite: we use the real logo at 32px, and apply a small bounce/blink mask.
function KookaSprite({ mood, wiggleAmt, chaos, generating }) {
  // layered: the logo + a tiny eye blink square + beak-tap on generating
  const eyeBlink = `koo-kooka-blink ${3 - chaos * 0.3}s steps(2) infinite`;

  return (
    <div style={{ position: 'relative', width: '100%', height: '100%' }}>
      <img
        src="assets/kookaburra.svg"
        alt=""
        style={{
          width: '100%', height: '100%',
          imageRendering: 'pixelated',
          display: 'block',
          animation: mood === 'cheering' ? 'koo-kooka-cheer 0.5s ease-in-out infinite'
            : generating ? 'koo-kooka-watch 1.8s ease-in-out infinite'
            : mood === 'laugh' ? 'koo-kooka-laugh 0.3s ease-in-out infinite'
            : 'none',
          transformOrigin: 'center bottom',
        }}
      />
      {/* simulated eye blink: small black square over the eye area */}
      <div style={{
        position: 'absolute',
        // eye sits around x=60%, y=28% on the pixel grid
        left: '62%', top: '28%',
        width: 3, height: 3,
        background: 'oklch(0.05 0 0)',
        animation: eyeBlink,
        pointerEvents: 'none',
      }}/>
    </div>
  );
}

// Host component: measures tile rects relative to grid, forwards to Kooka.
function KookaHost({ gridRef, workspaces, wsIdx, zen, focused, tiles, enabled, chatty, chaos }) {
  const [rects, setRects] = React.useState([]);
  const [overlayBox, setOverlayBox] = React.useState({ left: 0, top: 0, width: 0, height: 0 });

  const measure = React.useCallback(() => {
    if (!gridRef.current) return;
    const gridEl = gridRef.current;
    const parentBox = gridEl.offsetParent ? gridEl.offsetParent.getBoundingClientRect() : { left: 0, top: 0 };
    const gridBox = gridEl.getBoundingClientRect();
    setOverlayBox({
      left: gridBox.left - parentBox.left,
      top: gridBox.top - parentBox.top,
      width: gridBox.width,
      height: gridBox.height,
    });
    const children = Array.from(gridEl.children).filter(c => c.getAttribute('data-kooka') !== 'overlay');
    const out = children.map((c, i) => {
      const b = c.getBoundingClientRect();
      return {
        id: i,
        left: b.left - gridBox.left,
        top: b.top - gridBox.top,
        right: b.right - gridBox.left,
        bottom: b.bottom - gridBox.top,
      };
    });
    setRects(out);
  }, [gridRef]);

  React.useEffect(() => {
    measure();
    const t1 = setTimeout(measure, 80);
    const t2 = setTimeout(measure, 300);
    const onResize = () => measure();
    window.addEventListener('resize', onResize);
    return () => { clearTimeout(t1); clearTimeout(t2); window.removeEventListener('resize', onResize); };
  }, [measure, zen, wsIdx, tiles.length]);

  if (!enabled) return null;

  const generatingIdxs = tiles.map((tl, i) => tl.generating ? i : -1).filter(i => i >= 0);

  return (
    <div
      data-kooka="overlay"
      style={{
        position: 'absolute',
        left: overlayBox.left,
        top: overlayBox.top,
        width: overlayBox.width,
        height: overlayBox.height,
        pointerEvents: 'none',
        zIndex: 30,
      }}
    >
      <Kooka
        tileRects={rects}
        focused={focused}
        generatingIdxs={generatingIdxs}
        wsIdx={wsIdx}
        enabled={enabled}
        chaos={chaos}
        chatty={chatty}
      />
    </div>
  );
}

Object.assign(window, { Kooka, KookaHost });
