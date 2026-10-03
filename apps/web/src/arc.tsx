// Solid ports of UIArc's native Button, Card, Input and Badge surfaces.
// Native semantics, event handlers and disabled behavior are retained. See UIARC.md.
import { splitProps, type JSX } from 'solid-js';
import { Dynamic } from 'solid-js/web';
import './arc.css';
const classes = (base: string, name?: string, flags?: Record<string, boolean | undefined>) => [base, name, ...Object.entries(flags || {}).filter(([, enabled]) => enabled).map(([key]) => key)].filter(Boolean).join(' ');
export function ArcButton(props: JSX.ButtonHTMLAttributes<HTMLButtonElement>) {
 const [local, rest] = splitProps(props, ['class', 'classList']);
 return <button {...rest} class={classes('arc-button', local.class, local.classList)} />;
}
export function ArcInput(props: JSX.InputHTMLAttributes<HTMLInputElement>) {
 const [local, rest] = splitProps(props, ['class', 'classList']);
 return <input {...rest} class={classes(['checkbox', 'radio', 'range', 'color'].includes(props.type || '') ? '' : 'arc-input', local.class, local.classList)} />;
}
export function ArcBadge(props: JSX.HTMLAttributes<HTMLSpanElement>) {
 const [local, rest] = splitProps(props, ['class', 'classList']);
 return <span {...rest} class={classes('arc-badge', local.class, local.classList)} />;
}
export function ArcCard(props: JSX.HTMLAttributes<HTMLElement> & {as?: 'div' | 'article' | 'section'}) {
 const [local, rest] = splitProps(props, ['class', 'classList', 'as']);
 return <Dynamic component={local.as || 'section'} {...rest} class={classes('arc-card', local.class, local.classList)} />;
}
