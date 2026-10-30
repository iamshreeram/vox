// Vox orb avatar: inline SVG + CSS only. The orb is the Vox icon (globe with halo rings); it has no
// face. The face value is a pure function of `face` (see avatarFace in chatModel.ts); all motion is
// CSS keyframes in chat.css and avatar.css (transform/opacity). The globe's colours come from the
// per-instance custom properties in avatarStyle.ts (set inline on this svg), so the orb page, the
// conversation window and the Appearance preview paint the same globe. The SVG is decorative: the
// status pill and the transcript carry the meaning.
import { useId, type CSSProperties } from "react";
import type { AvatarFace } from "./chatModel";
import { SHINE } from "../orb/globePalette";
import { AUTO_AVATAR_LOOK, avatarStyle, type AvatarLook } from "./avatarStyle";

export default function VoxAvatar({
  face,
  look = AUTO_AVATAR_LOOK,
}: {
  face: AvatarFace;
  look?: AvatarLook;
}) {
  // One gradient pair per instance: two avatars on one page never share an id (ids hold no ':').
  const scope = useId().replace(/:/g, "");
  const bodyId = `vox-orb-${scope}`;
  const glowId = `vox-glow-${scope}`;
  const style = avatarStyle(face, look) as CSSProperties;
  return (
    <svg
      className="vox-avatar"
      data-face={face}
      data-orb-color={look.orbColor}
      aria-hidden="true"
      focusable="false"
      viewBox="0 0 160 160"
      style={style}
    >
      <defs>
        <radialGradient id={bodyId} cx="36%" cy="30%" r="75%">
          <stop offset="0" style={{ stopColor: "var(--vox-orb-hi)" }} />
          <stop offset="0.55" style={{ stopColor: "var(--vox-orb-mid)" }} />
          <stop offset="1" style={{ stopColor: "var(--vox-orb-lo)" }} />
        </radialGradient>
        <radialGradient id={glowId}>
          <stop offset="0" stopColor="currentColor" stopOpacity="0.38" />
          <stop offset="1" stopColor="currentColor" stopOpacity="0" />
        </radialGradient>
      </defs>
      <circle
        className="vox-glow"
        cx="80"
        cy="80"
        r="78"
        fill={`url(#${glowId})`}
      />
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
          fill={`url(#${bodyId})`}
        />
        <ellipse
          className="vox-shine"
          cx="66"
          cy="62"
          rx="15"
          ry="9"
          transform="rotate(-28 66 62)"
          fill={SHINE.colour}
          opacity={SHINE.opacity}
        />
      </g>
    </svg>
  );
}
