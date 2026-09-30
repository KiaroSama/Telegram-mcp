"""The admin-right sets per chat type, as Telegram Desktop 7.2.10 lists them (spec 033).

The expected lists below are copied by hand from `NestedAdminRightLabels` in
`boxes/peers/edit_peer_permissions_box.cpp` (tag v7.2.10) and the English strings of
`Resources/langs/lang.strings` of the same tag. They are deliberately NOT derived from the
module under test: a test that reads the answer from the code it checks cannot fail.
"""

import pytest
from telethon.tl.types import ChatAdminRights

from telegram_mcp import admin_rights_sets as sets

# (parent, [(telethon field, Desktop label), ...]) in Desktop's order. A parent of None
# means the rights sit at the top level of the screen.
GROUP = [
    (
        None,
        [
            ("change_info", "Change group info"),
            ("manage_welcome_messages", "Manage Welcome Messages"),
            ("delete_messages", "Delete messages"),
            ("ban_users", "Ban users"),
            ("invite_users", "Invite users via link"),
            ("manage_topics", "Manage topics"),
            ("pin_messages", "Pin messages"),
        ],
    ),
    (
        "Manage stories",
        [
            ("post_stories", "Post stories"),
            ("edit_stories", "Edit stories of others"),
            ("delete_stories", "Delete stories of others"),
        ],
    ),
    (
        None,
        [
            ("manage_call", "Manage video chats"),
            ("manage_ranks", "Edit member tags"),
            ("anonymous", "Remain anonymous"),
            ("add_admins", "Add new admins"),
        ],
    ),
]
CHANNEL = [
    (
        None,
        [
            ("change_info", "Change channel info"),
            ("manage_welcome_messages", "Manage Welcome Messages"),
        ],
    ),
    (
        "Manage messages",
        [
            ("post_messages", "Post messages"),
            ("edit_messages", "Edit messages of others"),
            ("delete_messages", "Delete messages of others"),
        ],
    ),
    (
        "Manage stories",
        [
            ("post_stories", "Post stories"),
            ("edit_stories", "Edit stories of others"),
            ("delete_stories", "Delete stories of others"),
        ],
    ),
    (
        None,
        [
            ("invite_users", "Add members"),
            ("manage_call", "Manage live streams"),
            ("manage_direct_messages", "Manage direct messages"),
            ("add_admins", "Add new admins"),
            ("ban_users", "Ban users"),
        ],
    ),
]
COMMUNITY = [
    (
        None,
        [
            ("change_info", "Edit Community Name"),
            ("manage_linked_peers", "Edit Group List"),
            ("ban_users", "Ban Members"),
            ("add_admins", "Add new admins"),
        ],
    ),
]


def _shape(kind, **options):
    return [
        (section.parent, [(right.field, right.label) for right in section.rights])
        for section in sets.sections(kind, **options)
    ]


def _without(expected, field):
    return [(parent, [pair for pair in pairs if pair[0] != field]) for parent, pairs in expected]


def test_a_forum_group_lists_desktops_group_rights_in_its_order():
    assert _shape("group", is_forum=True) == GROUP


def test_a_plain_group_has_no_topics_right():
    assert _shape("group", is_forum=False) == _without(GROUP, "manage_topics")


def test_a_channel_lists_desktops_channel_rights_in_its_order():
    assert _shape("channel") == CHANNEL


def test_a_community_lists_desktops_four_rights():
    assert _shape("community") == COMMUNITY


def test_the_welcome_label_and_the_invite_label_follow_the_chat_and_the_user():
    bot = _shape("group", is_forum=True, is_bot=True)
    assert ("manage_welcome_messages", "Send Welcome Messages") in bot[0][1]
    assert _shape("channel", is_bot=True)[0][1][1] == (
        "manage_welcome_messages",
        "Send Welcome Messages",
    )
    closed = _shape("group", is_forum=True, anyone_can_add=False)
    assert ("invite_users", "Add members") in closed[0][1]


def test_every_field_is_a_real_telethon_right():
    known = set(ChatAdminRights().to_dict()) - {"_"}
    for kind in sets.KINDS:
        assert set(sets.fields(kind, is_forum=True)) <= known, kind


def test_full_admin_is_every_right_of_the_type_except_anonymous():
    group = sets.full_admin("group", is_forum=True)
    assert group["anonymous"] is False
    assert group["add_admins"] is True
    assert group["manage_welcome_messages"] is True
    assert group["manage_topics"] is True
    assert set(group) == set(sets.fields("group", is_forum=True))
    assert all(on for name, on in group.items() if name != "anonymous")


def test_full_admin_of_a_plain_group_leaves_topics_out():
    assert "manage_topics" not in sets.full_admin("group", is_forum=False)


def test_full_admin_of_a_channel_holds_no_group_only_right():
    channel = sets.full_admin("channel")
    for foreign in ("pin_messages", "manage_topics", "manage_ranks", "anonymous"):
        assert foreign not in channel, foreign
    assert channel["manage_welcome_messages"] is True
    assert channel["manage_direct_messages"] is True


def test_full_admin_of_a_community():
    assert sets.full_admin("community") == {
        "change_info": True,
        "manage_linked_peers": True,
        "ban_users": True,
        "add_admins": True,
    }


def test_validate_accepts_a_right_of_the_type():
    assert sets.validate("channel", {"post_messages": True, "ban_users": False}) is None


def test_validate_names_the_chat_type_a_foreign_right_belongs_to():
    refusal = sets.validate("channel", {"pin_messages": True})
    assert "pin_messages" in refusal
    assert "Pin messages" in refusal
    assert "group" in refusal


def test_validate_names_every_type_that_has_the_right():
    refusal = sets.validate("community", {"delete_messages": True})
    assert "group" in refusal and "channel" in refusal


def test_validate_refuses_a_name_that_is_no_right_anywhere():
    refusal = sets.validate("group", {"nuke": True})
    assert "nuke" in refusal and "not an admin right" in refusal


def test_validate_refuses_topics_outside_a_forum():
    refusal = sets.validate("group", {"manage_topics": True}, is_forum=False)
    assert "forum" in refusal
    assert sets.validate("group", {"manage_topics": True}, is_forum=True) is None


def test_validate_reports_the_right_desktop_shows_but_layer_229_lacks():
    refusal = sets.validate("group", {"process_join_requests": True})
    assert "not available at this Telegram layer" in refusal


def test_promoting_starts_from_full_admin_and_applies_what_was_named():
    rights = sets.effective_rights(
        "group", {"ban_users": False, "anonymous": True}, is_forum=False, exact=False
    )
    assert rights["ban_users"] is False
    assert rights["anonymous"] is True
    assert rights["manage_welcome_messages"] is True
    assert "manage_topics" not in rights


def test_editing_grants_exactly_what_was_named():
    rights = sets.effective_rights(
        "channel", {"post_messages": True, "edit_messages": None}, is_forum=False, exact=True
    )
    assert rights == {**{name: False for name in sets.fields("channel")}, "post_messages": True}


def test_the_permissions_line_of_a_full_channel_admin():
    line = sets.permissions_line("channel", sets.full_admin("channel"))
    assert line == (
        "permissions: Change channel info | Manage Welcome Messages"
        " | Manage messages: Post messages, Edit messages of others, Delete messages of others"
        " | Manage stories: Post stories, Edit stories of others, Delete stories of others"
        " | Add members | Manage live streams | Manage direct messages | Add new admins"
        " | Ban users"
    )


def test_the_permissions_line_of_a_full_forum_admin_with_the_bot_and_invite_wording():
    line = sets.permissions_line(
        "group", sets.full_admin("group", is_forum=True), is_forum=True, is_bot=True
    )
    assert line == (
        "permissions: Change group info | Send Welcome Messages | Delete messages | Ban users"
        " | Invite users via link | Manage topics | Pin messages"
        " | Manage stories: Post stories, Edit stories of others, Delete stories of others"
        " | Manage video chats | Edit member tags | Add new admins"
    )


def test_the_permissions_line_lists_only_granted_rights_and_drops_an_empty_parent():
    line = sets.permissions_line(
        "channel", {"post_messages": True, "delete_messages": True, "ban_users": True}
    )
    assert line == (
        "permissions: Manage messages: Post messages, Delete messages of others | Ban users"
    )


def test_the_permissions_line_of_nothing_says_so():
    assert sets.permissions_line("community", {}) == "permissions: none"


def test_the_community_line_uses_its_own_words():
    assert (
        sets.permissions_line("community", sets.full_admin("community"))
        == "permissions: Edit Community Name | Edit Group List | Ban Members | Add new admins"
    )


@pytest.mark.parametrize("kind", ["group", "channel", "community"])
def test_a_rights_dict_builds_a_telethon_object_carrying_only_those_rights(kind):
    built = sets.to_telethon(sets.full_admin(kind, is_forum=True))
    assert isinstance(built, ChatAdminRights)
    assert built.anonymous is False
    granted = {name for name, on in built.to_dict().items() if on is True}
    assert granted == {name for name, on in sets.full_admin(kind, is_forum=True).items() if on}
