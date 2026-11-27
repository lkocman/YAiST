# Hackweek 26 plan: the first three months with openSUSE

**When:** Monday 30 November to Friday 4 December 2026. Tuesday 1 December
is off (St Nicholas); its work, the ready-to-boot image, moves to the
weekend of 28–29 November.
New pull requests get the `Hackweek26` label.

**Goal:** go through every module again from the point of view of someone new
to Linux. Keep what helps them, drop what doesn't, and add what is missing, so
that their first three months on openSUSE go smoothly and end with them
taking a first step towards contributing.

## Who we build for

**Alex** is coming from Windows (or from Ubuntu after a friend's
recommendation). They have a laptop with Intel and NVIDIA graphics, a home
printer, a Steam library and a phone full of photos. They have never opened a
terminal on purpose. They are happy to learn, as long as nothing breaks
without a way back.

The journey this week follows:

| Stage | Alex wants to… | Modules |
|---|---|---|
| Day 0–1 | get set up: updates, drivers, codecs, the apps they know | Welcome, System, Packages, Gaming & media, AI assistant |
| Week 1–4 | everyday use: find apps, add a repo, print, share the computer | Packages, Repositories, Printers, Users |
| Month 1–2 | recover when something goes wrong, and understand why | Snapshots, Firewall, SELinux, problem reports |
| Month 3 | upgrade to the next release, and give something back | Upgrade, Community |

## Every module, every day: the same checklist

For each module, on Tumbleweed, Leap 16.0 and one immutable system (Aeon, or
Leap Micro for the server side), using clean containers
(`scripts/clean-container.sh`) or a VM for anything that needs systemd or real
hardware:

1. **First look.** Does the panel make sense to Alex without the handbook?
   Are the words theirs or ours ("zone", "boolean", "vendor change")?
2. **Do the three most common jobs** from the panel, then the same jobs in
   the conversation, and once more with the assistant off. All three paths
   must work.
3. **Break it on purpose:** no network, wrong password, cancelled polkit
   prompt, an already-done change, a missing package. Is the error something
   Alex can act on?
4. **Keep, drop or add.** For each action, ask whether Alex needs it in the
   first three months. Expert-only actions can stay, but out of the way.
5. **Teach.** Does the plan card show the command, so that Alex learns the
   terminal along the way?
6. **Write down** the result in the scorecard below. Fix small things
   straight away; bigger ones become issues.

Each change follows CLAUDE.md as usual: the handbook in the same pull request,
before/after screenshots, a branch and a pull request.

## The plan

### Weekend (28–29 Nov): groundwork, and Tuesday's image

- [ ] Set up the test machines: a Tumbleweed VM, a Leap 16.0 VM (16.1 is
      out, so the upgrade path is real) and an Aeon VM. Snapshot each one
      fresh after install, so that every day starts from a clean first boot.
- [ ] Do a cold run as Alex: install, log in, open YAiST, and give yourself
      30 minutes to "get set up". Note every place you got stuck. This is the
      baseline for Friday.
- [ ] Build the ready-to-boot image (Tuesday's work, see below).
- [ ] Merge #28 (drop Services). Look at #22 (table columns truncating),
      since every module's table runs into it.
- [ ] Optional: write 20–30 newbie questions in Alex's words (e.g. "my
      wifi doesn't work", "how do I install Spotify", "is this safe?"). They
      are both the test set for the week and the welcome screen's prompts.

### Monday 30 Nov: Day 0, the first boot

*Alex logs in for the first time. Within 15 minutes, without a terminal, the
system should be up to date, with drivers and codecs installed and the
assistant either ready or politely set aside.*

- [ ] **Welcome:** is the first screen clear about what to do first? Order
      the "get started" cards by what a fresh install needs (updates, then
      drivers and codecs, then apps). Does it replace openSUSE-welcome
      without losing anything?
- [ ] **Packages, updates:** the first big update. Does the progress make
      sense, and does a reboot get asked for when it's needed?
- [ ] **Gaming & media:** codecs, the NVIDIA driver (including Secure Boot
      and MOK enrolment: what does Alex see on the next boot?), Steam.
- [ ] **AI assistant:** the first-run choice. Opting out has to be easy and
      final (the remaining part of the optional-assistant plan: the user
      preference, "never ask again"). Old hardware gets told once, kindly.
      CPU mode shows the "SLOWER" label.
- [ ] **System:** hostname, time zone and NTP. Does Alex need anything here
      on day 0, or does this module belong further down the sidebar?
- [ ] Decide: the order of the sidebar for a newcomer.

### Tuesday 1 Dec: St Nicholas 🎅, the ready-to-boot image (done at the weekend)

Off. The image is Tuesday's work, done over the weekend of 28–29 November
instead.

*Someone curious about YAiST downloads one file, imports it into
virt-manager and boots straight into YAiST, assistant included, without
installing anything on their own machine.*

- [ ] **Base it on our OEM image** and build it with KIWI on OBS, in
      `home:lkocman:YAiST` next to the `yaist` and `yaist-offline` packages,
      so that it rebuilds when they change. Start from Leap 16.0 with
      one desktop: 16.1 is out, so the demo shows the banner for the newer
      Leap and the upgrade path for real. Other flavours can come later. The
      output is a qcow2 file for VMs; because it's an OEM image, the same
      build can later go on a USB stick too.
- [ ] **Boot right into it:** log a demo user in automatically and open
      YAiST on the welcome screen. Preinstall the model (`yaist-offline`,
      about 2.5 GB), so that the assistant works without downloading
      anything.
- [ ] **The assistant in a VM:** what runs where.
      - *CPU mode*, the default that works everywhere. It needs the
        `host-passthrough` CPU model (for AVX2), 4 or more vCPUs and 8 GB
        of RAM. It's slow but it works, and it shows the "SLOWER" label.
      - *GPU passthrough* (VFIO) for hosts with a spare GPU and IOMMU
        turned on. This is the real speed. Document it as advanced.
      - *virtio-gpu with Venus* (Vulkan in the guest). It's experimental:
        try whether llama.cpp's Vulkan backend runs on it. If it does, it's
        GPU speed without passthrough.
- [ ] **Use it as the test machine** from Wednesday on: a fresh image every
      morning is the clean first boot that the checklist needs.
- [ ] **Instructions for virt-manager:** a page in `docs/`, linked from the
      README.
      1. Download the image and check its checksum.
      2. *New VM → Import existing disk image*, OS openSUSE.
      3. Memory 8 GB, 4 or more CPUs, then *Customize configuration before
         install*.
      4. Firmware UEFI, CPU model `host-passthrough`, Video Virtio
         (3D acceleration only when trying Venus), Display Spice.
      5. Optional: *Add Hardware → PCI Host Device* for a passed-through GPU,
         with the IOMMU and vfio-pci steps it needs, linked rather than
         copied.
      6. The same thing with `virt-install`, for people who prefer a
         single command.
- [ ] Decide: what the image says on first boot. It's a demo, so it should
      say what YAiST is and that changes only happen inside the VM.

### Wednesday 2 Dec: Weeks 1–4, everyday use

*Alex installs the apps they know, adds a repository a forum post told them
about, prints their first document and makes an account for a family member.*

- [ ] **Packages:** search finds what Alex calls things ("photoshop" → GIMP,
      "spotify", "zoom", "office"). Flatpak versus RPM: is the choice
      explained, or made for them?
- [ ] **Repositories:** adding an OBS or third-party repository. Is the
      trust prompt understandable? Does it warn about vendor changes and
      mixing repositories, the way newcomers usually break Tumbleweed?
- [ ] **Printers:** a driverless IPP printer and an HP one. "My printer
      doesn't show up" needs an answer.
- [ ] **Users:** an account for a child or a guest, without sudo. "Admin
      rights" explained in one sentence.
- [ ] Add, if missing: "what's taking my disk space", and wifi/Bluetooth
      pointers (even if only to the desktop's own settings, so that YAiST
      doesn't leave Alex with no answer).

### Thursday 3 Dec: Months 1–2, when something goes wrong

*An update breaks something, an app can't connect, a game won't start. Alex
needs a way back and an explanation, not a lecture.*

- [ ] **Snapshots:** "it worked yesterday" → compare and roll back, from the
      panel and from the conversation. Test it for real in a VM: the reboot,
      `snapper rollback`, and what Alex sees afterwards. Immutable systems
      too.
- [ ] **Firewall:** "my game/server/KDE Connect can't connect". Should the
      common cases (KDE Connect, Steam Remote Play, Samba) be named presets
      instead of ports? Hide zones from newcomers?
- [ ] **SELinux:** a denial explained in plain words, with the boolean as
      the fix. Is this module for Alex at all, or should it only appear when
      a denial happens?
- [ ] **Problem reports:** the "report this" path, for openSUSE and for
      YAiST. Does it collect the right logs, and is it clear where the report
      goes?
- [ ] The assistant on its bad days: a wrong tool call, no model running,
      a slow first reply. Is the panel path always there as a fallback?

### Friday 4 Dec: Month 3, upgrade and give back

*Leap 16.1 is out. Alex upgrades without fear, then gets curious about the
people behind openSUSE.*

- [ ] **Upgrade:** Leap 16.0 → 16.1 for real in a VM, from the banner
      (merged in #26) to the first boot afterwards: the release notes, the
      snapshot before it, what to do when it fails halfway.
      Tumbleweed/Slowroll: what `dup` means and when to run it.
- [ ] **Community, the contributor ladder:** each step one click away and
      shown when it makes sense, not all on day 1:
      1. say thanks (kudos) and join the chat or forums,
      2. report a bug well (already in YAiST),
      3. fix a typo in the docs or the handbook,
      4. test a package from OBS, then branch one,
      5. come to an event or a Hackweek.
      Should YAiST offer step 1 once, around month three ("you've been using
      openSUSE for 90 days")?
- [ ] **Image:** publish the ready-to-boot image with its virt-manager
      instructions, and boot it once on a machine that hasn't seen YAiST.
- [ ] Redo the weekend's cold run. Compare with the baseline: time to set
      up, number of places stuck.
- [ ] Wrap-up: regenerate the screenshots (`scripts/screenshots.sh`), update
      the Hackweek project page with a short demo, and record what's left in
      issues.

## Scorecard

Fill in as the week goes. Verdict: **keep** / **rework** / **hide** (expert
only) / **drop**.

| Module | Day | TW | Leap | Immutable | Verdict | Notes, issues, PRs |
|---|---|---|---|---|---|---|
| Welcome | Mon | | | | | |
| System | Mon | | | | | |
| Packages | Mon/Wed | | | | | |
| Gaming & media | Mon | | | | | |
| AI assistant | Mon | | | | | |
| Repositories | Wed | | | | | |
| Printers | Wed | | | | | |
| Users | Wed | | | | | |
| Snapshots | Thu | | | | | |
| Firewall | Thu | | | | | |
| SELinux | Thu | | | | | |
| Problem reports | Thu | | | | | |
| Upgrade | Fri | | | | | |
| Community | Fri | | | | | |
| Ready-to-boot image | Tue (weekend) | | | | | |
| ~~Services~~ | | | | | drop | #28 |

## Not this week

Things that would pull focus. Note them, don't start them:

- `yaistd`, the privileged daemon.
- A local knowledge base from the SUSE documentation (doc-modular sources;
  check the licence first).
- Splitting the `yaist` / `yaist-assistant` packages.
- New modules for experts.
