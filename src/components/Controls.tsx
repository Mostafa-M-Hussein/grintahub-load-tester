interface ControlsProps {
  isRunning: boolean;
  onStart: () => void;
  onStop: () => void;
  disabled?: boolean;
}

export function Controls({ isRunning, onStart, onStop, disabled }: ControlsProps) {
  return (
    <div className="controls">
      {isRunning ? (
        <button
          className="control-btn stop"
          onClick={onStop}
          disabled={disabled}
        >
          Stop Bot
        </button>
      ) : (
        <button
          className="control-btn start"
          onClick={onStart}
          disabled={disabled}
        >
          Start Bot
        </button>
      )}
    </div>
  );
}
