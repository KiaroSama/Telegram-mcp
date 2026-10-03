"""Capabilities shared by approval and export dialogs."""


def supports_form(session) -> bool:
    caps = getattr(session, "client_capabilities", None)
    elicitation = getattr(caps, "elicitation", None)
    if elicitation is None:
        return False
    # Legacy elicitation:{} means form-only; URL-only never supports a form.
    return (
        getattr(elicitation, "form", None) is not None or getattr(elicitation, "url", None) is None
    )
