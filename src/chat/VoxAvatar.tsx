// Vox orb avatar: inline SVG + CSS only. The orb is the Vox icon (gold sphere with halo rings);
// it has no face. The face value is a pure function of `face` (see avatarFace in chatModel.ts);
// all motion is CSS keyframes in chat.css (transform/opacity). The SVG is decorative: the
// status pill and the transcript carry the meaning. Halo, glow, arc and dots take the state
// accent through var(--chat-accent) (set on .chat-root from chatTheme.accentStyle).
import type { AvatarFace } from "./chatModel";

export default function VoxAvatar({ face }: { face: AvatarFace }) {
  return (
    <svg
      className="vox-avatar"
      data-face={face}
      aria-hidden="true"
      focusable="false"
      viewBox="0 0 160 160"
    >
      <defs>
        <radialGradient id="vox-orb" cx="36%" cy="30%" r="75%">
          <stop offset="0" stopColor="#FFE27A" />
          <stop offset="0.55" stopColor="#F5B921" />
          <stop offset="1" stopColor="#B97A00" />
        </radialGradient>
        <radialGradient id="vox-glow-fill">
          <stop offset="0" stopColor="currentColor" stopOpacity="0.38" />
          <stop offset="1" stopColor="currentColor" stopOpacity="0" />
        </radialGradient>
      </defs>
      <circle className="vox-glow" cx="80" cy="80" r="78" />
      <circle className="vox-ring vox-ring-1" cx="80" cy="80" r="50" />
      <circle className="vox-ring vox-ring-2" cx="80" cy="80" r="60" />
      <circle className="vox-ring vox-ring-3" cx="80" cy="80" r="70" />
      <circle className="vox-arc" cx="80" cy="80" r="66" />
      <g className="vox-dots">
        <circle cx="80" cy="10" r="3.5" />
        <circle cx="140.6" cy="115" r="3.5" />
        <circle cx="19.4" cy="115" r="3.5" />
      </g>
      <g className="vox-body-group">
        <circle
          className="vox-body"
          cx="80"
          cy="80"
          r="42"
          fill="url(#vox-orb)"
        />
        <ellipse
          className="vox-shine"
          cx="66"
          cy="62"
          rx="15"
          ry="9"
          transform="rotate(-28 66 62)"
        />
      </g>
    </svg>
  );
}
