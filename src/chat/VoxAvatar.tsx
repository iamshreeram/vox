// Original "orb" avatar: inline SVG + CSS only. The face is a pure function of `face`
// (see avatarFace in chatModel.ts); all motion is CSS keyframes in chat.css (transform/opacity).
// The SVG is decorative: the status caption and the transcript carry the meaning.
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
        <radialGradient id="vox-body-fill" cx="40%" cy="34%" r="72%">
          <stop offset="0" className="vox-stop-hi" />
          <stop offset="1" className="vox-stop-lo" />
        </radialGradient>
      </defs>
      <circle className="vox-glow" cx="80" cy="80" r="74" />
      <circle className="vox-ring" cx="80" cy="80" r="66" />
      <circle className="vox-arc" cx="80" cy="80" r="66" />
      <g className="vox-float">
        <g className="vox-pulse">
          <circle
            className="vox-body"
            cx="80"
            cy="82"
            r="48"
            fill="url(#vox-body-fill)"
          />
          <g className="vox-eyes">
            <ellipse
              className="vox-eye vox-eye-l"
              cx="63"
              cy="76"
              rx="7"
              ry="9"
            />
            <ellipse
              className="vox-eye vox-eye-r"
              cx="97"
              cy="76"
              rx="7"
              ry="9"
            />
          </g>
          <g className="vox-closed">
            <path d="M56 78 q7 6 14 0" />
            <path d="M90 78 q7 6 14 0" />
          </g>
          <g className="vox-brows">
            <path className="vox-brow vox-brow-l" d="M55 62 l14 -6" />
            <path className="vox-brow vox-brow-r" d="M105 62 l-14 -6" />
          </g>
          <path className="vox-mouth" d="M66 102 q14 10 28 0" />
        </g>
      </g>
      <g className="vox-dots">
        <circle cx="80" cy="8" r="4" />
        <circle cx="134" cy="112" r="4" />
        <circle cx="26" cy="112" r="4" />
      </g>
      <path className="vox-z" d="M124 26 h10 l-10 12 h10" />
    </svg>
  );
}
