--!strict
-- azdo, window half: palette commands that open the tern-azdo binary in a
-- new tab or beside the focused pane.

local GROUP = "Azure DevOps"

local function launch(cx: WindowCx, how: "tab" | "split")
	if how == "split" then
		local pane = cx.session:focused()
		if pane then
			cx.layout:split(pane, "right", { command = "tern-azdo" })
			return
		end
	end
	cx.layout:new_tab({ command = "tern-azdo" })
end

tern.command({
	id = "open",
	title = "Azure DevOps: Pull Requests",
	icon = "git-pull-request",
	group = GROUP,
	run = function(cx)
		launch(cx, "tab")
	end,
})

tern.command({
	id = "open_split",
	title = "Azure DevOps: Pull Requests Beside Current Pane",
	icon = "git-pull-request",
	group = GROUP,
	run = function(cx)
		launch(cx, "split")
	end,
})
