# UIArc component provenance

The Button, Card, Input and Badge CSS is adapted from the free MIT registry at
https://github.com/kuratlielia/arc-library/tree/792791245398f1009a0054544a02fb4f3455df07/registry/components
(component source files named `<component>.module.css`). Design reference: https://uiarc.dev/.

`arc.tsx` ports these surfaces to Solid and retains native browser controls and the existing application handlers. `arc.css` copies the base/variant rules and namespaces their selectors and foundation tokens. Only used surfaces are included; React/Motion quick-look and morph animations are omitted. Existing application fonts, blue accent, dark-mode support where available, and density override the foundation. Visible keyboard focus is deliberately retained, unlike the upstream foundation reset. Reduced-motion users get immediate state changes. No Pro component or new runtime dependency is included.

The accompanying UIARC-LICENSE preserves the upstream copyright and MIT terms.

The production static assets include the same notice at `/licenses/UIArc.txt`.
