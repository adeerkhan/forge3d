import { useEffect, useId, useRef, useState, type ReactNode } from "react";

interface NumberFieldProps {
  label: string;
  value: number;
  step?: number;
  min?: number;
  max?: number;
  disabled?: boolean;
  onCommit(value: number): void;
}

/**
 * Numeric input that commits on blur or Enter and reverts on Escape.
 *
 * Editing must not fire a command per keystroke: that would flood the undo
 * stack and autosave with half-typed values.
 */
export function NumberField({
  label,
  value,
  step = 0.1,
  min,
  max,
  disabled,
  onCommit,
}: NumberFieldProps) {
  const id = useId();
  const [draft, setDraft] = useState(() => format(value));
  const focused = useRef(false);

  useEffect(() => {
    if (!focused.current) setDraft(format(value));
  }, [value]);

  const commit = () => {
    focused.current = false;
    const parsed = Number.parseFloat(draft);
    if (!Number.isFinite(parsed)) {
      setDraft(format(value));
      return;
    }
    const clamped = Math.min(max ?? Number.POSITIVE_INFINITY, Math.max(min ?? Number.NEGATIVE_INFINITY, parsed));
    if (Math.abs(clamped - value) > 1e-9) onCommit(clamped);
    setDraft(format(clamped));
    if (clamped !== parsed) {
      // Show the value the model actually accepted.
      setDraft(format(clamped));
    }
  };

  return (
    <label className="field" htmlFor={id}>
      <span className="field-label">{label}</span>
      <input
        id={id}
        className="input input--number"
        type="number"
        inputMode="decimal"
        step={step}
        min={min}
        max={max}
        disabled={disabled}
        value={draft}
        onChange={(event) => setDraft(event.target.value)}
        onFocus={() => {
          focused.current = true;
        }}
        onBlur={commit}
        onKeyDown={(event) => {
          if (event.key === "Enter") {
            event.currentTarget.blur();
          } else if (event.key === "Escape") {
            focused.current = false;
            setDraft(format(value));
            event.currentTarget.blur();
          }
        }}
      />
    </label>
  );
}

interface RangeFieldProps {
  label: string;
  value: number;
  min?: number;
  max?: number;
  step?: number;
  disabled?: boolean;
  onCommit(value: number): void;
}

/**
 * Slider that previews while dragging and commits once on release.
 *
 * Committing per input event would push one undo entry per pixel of the drag;
 * the published value is what the model accepts, and the drag still reads
 * live in the label. Keyboard use commits on key-up.
 */
export function RangeField({
  label,
  value,
  min = 0,
  max = 1,
  step = 0.01,
  disabled,
  onCommit,
}: RangeFieldProps) {
  const id = useId();
  const [draft, setDraft] = useState(value);
  const dragging = useRef(false);

  useEffect(() => {
    // A model update (undo, project open) wins once the pointer is released.
    if (!dragging.current) setDraft(value);
  }, [value]);

  const publish = (next: number) => {
    const clamped = Math.min(max, Math.max(min, Number.isFinite(next) ? next : value));
    setDraft(clamped);
    if (Math.abs(clamped - value) > 1e-9) onCommit(clamped);
  };

  return (
    <label className="field field--range" htmlFor={id}>
      <span className="field-label">{label}</span>
      <div className="field-range">
        <input
          id={id}
          type="range"
          min={min}
          max={max}
          step={step}
          value={draft}
          disabled={disabled}
          onPointerDown={() => {
            dragging.current = true;
          }}
          onChange={(event) => setDraft(Number(event.target.value))}
          onPointerUp={(event) => {
            dragging.current = false;
            publish(Number(event.currentTarget.value));
          }}
          onKeyUp={(event) => publish(Number(event.currentTarget.value))}
          onBlur={(event) => {
            dragging.current = false;
            publish(Number(event.currentTarget.value));
          }}
        />
        <span className="field-range__value mono">{draft.toFixed(2)}</span>
      </div>
    </label>
  );
}

interface TextFieldProps {
  label: string;
  value: string;
  disabled?: boolean;
  onCommit(value: string): void;
}
export function TextField({ label, value, disabled, onCommit }: TextFieldProps) {
  const id = useId();
  const [draft, setDraft] = useState(value);
  const focused = useRef(false);

  useEffect(() => {
    if (!focused.current) setDraft(value);
  }, [value]);

  const commit = () => {
    focused.current = false;
    const trimmed = draft.trim();
    if (!trimmed || trimmed === value) {
      setDraft(value);
      return;
    }
    onCommit(trimmed);
  };

  return (
    <label className="field" htmlFor={id}>
      <span className="field-label">{label}</span>
      <input
        id={id}
        className="input"
        type="text"
        value={draft}
        disabled={disabled}
        onChange={(event) => setDraft(event.target.value)}
        onFocus={() => {
          focused.current = true;
        }}
        onBlur={commit}
        onKeyDown={(event) => {
          if (event.key === "Enter") event.currentTarget.blur();
          if (event.key === "Escape") {
            focused.current = false;
            setDraft(value);
            event.currentTarget.blur();
          }
        }}
      />
    </label>
  );
}

export function Section({ title, children, actions }: { title: string; children: ReactNode; actions?: ReactNode }) {
  return (
    <section className="inspector-section">
      <header className="inspector-section__header">
        <h3>{title}</h3>
        {actions}
      </header>
      {children}
    </section>
  );
}

export function Toggle({
  label,
  checked,
  onChange,
}: {
  label: string;
  checked: boolean;
  onChange(checked: boolean): void;
}) {
  const id = useId();
  return (
    <label className="toggle" htmlFor={id}>
      <input
        id={id}
        type="checkbox"
        checked={checked}
        onChange={(event) => onChange(event.target.checked)}
      />
      <span className="toggle__track" aria-hidden="true">
        <span className="toggle__thumb" />
      </span>
      <span className="toggle__label">{label}</span>
    </label>
  );
}

function format(value: number): string {
  if (!Number.isFinite(value)) return "0";
  const rounded = Math.round(value * 1000) / 1000;
  return Object.is(rounded, -0) ? "0" : String(rounded);
}
