export default function ChoiceButton({
  id,
  title,
  detail,
  action,
  selected,
  danger = false,
}: {
  id: string;
  title: string;
  detail?: string;
  action: () => void;
  selected?: boolean;
  danger?: boolean;
}) {
  return (
    <button
      id={id}
      type="button"
      className={'choice' + (danger ? ' danger' : '')}
      onClick={action}
      aria-pressed={selected}
    >
      <span>
        <span className="choice-name">{title}</span>
        {detail ? <span className="choice-detail">{detail}</span> : null}
      </span>
      {selected ? (
        <span className="check" aria-hidden="true">
          ✓
        </span>
      ) : null}
    </button>
  );
}
