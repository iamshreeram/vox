import React, { useRef } from "react";
import { Check } from "lucide-react";
import { nextRadioIndex, rovingTabStops } from "./appearanceModel";

export interface RadioChoice<V extends string> {
  value: V;
  label: string;
  /** One-line description shown under the label. */
  hint?: string;
}

interface AppearanceRadioGroupProps<V extends string> {
  /** Already translated; the group's accessible name. */
  label: string;
  choices: readonly RadioChoice<V>[];
  value: V;
  onChange: (value: V) => void;
  /** Stable prefix for the test ids of each radio. */
  testIdPrefix: string;
}

/**
 * A roving radio group with visible labels: arrow keys move and select, Home and End jump to the
 * ends, and only the selected radio is a tab stop. The selected radio shows a check mark and a
 * ring, so the state is not carried by colour alone.
 */
export function AppearanceRadioGroup<V extends string>({
  label,
  choices,
  value,
  onChange,
  testIdPrefix,
}: AppearanceRadioGroupProps<V>): React.ReactElement {
  const refs = useRef<(HTMLButtonElement | null)[]>([]);
  const selected = choices.findIndex((choice) => choice.value === value);
  const stops = rovingTabStops(selected, choices.length);

  const move = (index: number, key: string): void => {
    const next = nextRadioIndex(key, index, choices.length);
    if (next === null) return;
    const target = choices[next];
    refs.current[next]?.focus();
    onChange(target.value);
  };

  return (
    <div role="radiogroup" aria-label={label} className="flex flex-wrap gap-2">
      {choices.map((choice, index) => {
        const isSelected = index === selected;
        return (
          <button
            key={choice.value}
            ref={(node) => {
              refs.current[index] = node;
            }}
            type="button"
            role="radio"
            aria-checked={isSelected}
            tabIndex={stops[index]}
            data-testid={`${testIdPrefix}-${choice.value}`}
            onClick={() => {
              if (!isSelected) onChange(choice.value);
            }}
            onKeyDown={(event) => {
              if (nextRadioIndex(event.key, index, choices.length) === null) {
                return;
              }
              event.preventDefault();
              move(index, event.key);
            }}
            className={[
              "flex min-w-[10rem] flex-1 flex-col items-start gap-0.5 rounded-lg border px-3 py-2 text-start cursor-pointer",
              "transition-colors motion-reduce:transition-none focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-(--focus-ring)",
              "focus-visible:inset-ring-2 focus-visible:inset-ring-(--color-on-accent)",
              isSelected
                ? "border-accent-a bg-accent-soft ring-2 ring-accent-a"
                : "border-mid-gray/30 hover:bg-mid-gray/10",
            ].join(" ")}
          >
            <span className="flex items-center gap-2 text-sm font-medium">
              <span
                aria-hidden="true"
                className={`inline-flex h-4 w-4 items-center justify-center rounded-full border ${
                  isSelected
                    ? "border-accent-a bg-accent-a text-on-accent"
                    : "border-mid-gray/50"
                }`}
              >
                {isSelected ? <Check size={12} strokeWidth={3} /> : null}
              </span>
              {choice.label}
            </span>
            {choice.hint ? (
              <span
                className={`text-xs ${isSelected ? "text-accent-text" : "text-muted"}`}
              >
                {choice.hint}
              </span>
            ) : null}
          </button>
        );
      })}
    </div>
  );
}
