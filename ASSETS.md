# Assets

rustora ships only files that it may redistribute under the GNU GPL. OpenRA declares its whole repository GPL,
including the mod folders, but many binary files there are original Westwood artwork and sounds, fixes or edits of
them, or files of unclear origin. Those stay copyrighted by EA or their authors whatever the repository says.
rustora does not ship them. The content installer of each mod downloads them from the OpenRA source archive of
the matching tag, which OpenRA itself distributes:

- download: `https://github.com/OpenRA/OpenRA/archive/refs/tags/release-20250330.zip`, SHA1
  `60e8235c8e654479f64218792987b9f0807e55fe` (36 MB; two downloads gave the same SHA1);
- installed to `<support dir>/Content/openra-assets/release-20250330/`, laid out like the engine directory;
- a folder of the engine directory looks up its files there first (`rustora_fs::assets`), so mod manifests and
  chrome need no changes.

The required package "OpenRA Assets" (`ContentPackage@openra-assets`) is added to `ra-content` and `cnc-content`
by `scripts/update-mod-data.sh`. Quick Install downloads it after the game content. Folders that only held
downloaded files keep a `README.md`.

This audit is not a legal opinion. When the origin of a file was unclear, it was treated as not shippable.

## Method

- All 844 binary files of `mods/{common,common-content,ra,ra-content,cnc,cnc-content}` (excluding YAML, Lua,
  Fluent, map YAML, `map.bin` and `map.png`) plus `global mix database.dat`, with their full git history
  (`git log --follow`) in the OpenRA repository.
- Commit messages and authors name the origin: "noticable SHP fixes from C&C 1.06c", "more SHP fixes by Nyerguds",
  "crate effects from Sole Survivor", "sounds extracted from the c&c demo", "added Giant Ant from Counterstrike",
  "Use original RA and TD footprint artwork", terrain conversions, husks and shadows edited into original sprites,
  and community sprites without a stated license.
- The chrome atlases were inspected visually: they contain the Allied, Soviet, GDI, Nod and EVA logos. The EVA
  database images of cnc are stills from the original game.
- Fonts: FreeSans is GNU FreeFont (GPL). ZoodRangmah states "All rights reserved" and no license.

## Result

| Files | Verdict | Reason |
| --- | --- | --- |
| `mods/ra/bits/**`, `mods/cnc/bits/**` (752 files) | download | Westwood originals and edits, community art of unclear license |
| `mods/ra/uibits/**`, `mods/cnc/uibits/**` | download | faction and EVA logos, EVA database stills |
| `mods/ra/icon*.png`, `mods/cnc/icon*.png` | download | game logos |
| `mods/ra/ZoodRangmah.ttf` | download | no license |
| sprites and palettes in `ra/maps/{fort-lonestar,bomber-john,chernobyl}` | download | unclear origin |
| `mods/common-content/chrome*.png` | ship a copy with a black background, download the original | the background shows the faction logos; the installer needs the rest before any download |
| `global mix database.dat` | download | compiled with XCC, no license statement; only lists MIX contents |
| `mods/common/FreeSans*.ttf` | ship | GPL font |
| `mods/common-content/cursor.png` | ship | plain arrow made for OpenRA |
| ambient sounds of `fort-lonestar` and `gdi06` | ship | CC0, OpenRA commit befdac8237 |
| YAML, Lua, Fluent, maps | ship | OpenRA code and data under the GPL |

Mission maps recreate the layouts of the original Westwood missions, and texts such as unit names and briefings
follow the originals. They stay in the package as OpenRA's GPL work (decided 2026-11-11).

## Files

| File | First commit in OpenRA | Verdict |
| --- | --- | --- |
| `global mix database.dat` | 2013-05-18 Matthias Mailänder - replace PackageContent yamls with XCC database | download |
| `mods/cnc/bits/a10cr.tem` | 653d72086c 2012-07-04 Matthias Mailänder - A10 crash site to terrain | download |
| `mods/cnc/bits/afld.shp` | f7c69b4801 2015-05-08 reaperrr - Split radar dish from Nod airfield and make it an idle overlay | download |
| `mods/cnc/bits/afld_d.shp` | f7c69b4801 2015-05-08 reaperrr - Split radar dish from Nod airfield and make it an idle overlay | download |
| `mods/cnc/bits/allyrepair.shp` | be9948426c 2011-09-10 Curtis S - 1078: Added ally repairing for CNC and RA | download |
| `mods/cnc/bits/apcmuz.shp` | 5fceaaec58 2011-06-28 Paul Chote - New APC turret; art by Daniel Hernandez. | download |
| `mods/cnc/bits/apctur.shp` | 5fceaaec58 2011-06-28 Paul Chote - New APC turret; art by Daniel Hernandez. | download |
| `mods/cnc/bits/atomicon.shp` | 725f690aaa 2014-03-20 Paul Chote - Add posters for TD airstrike and nuke. | download |
| `mods/cnc/bits/beaconclock.shp` | e1501d5b7c 2014-05-31 Paul Chote - Add clock overlay to support power beacons. | download |
| `mods/cnc/bits/boaticnh.shp` | 008b0cbab5 2013-03-29 Matthias Mailänder - Added @Nyerguds TD Deluxe Cameo Pack | download |
| `mods/cnc/bits/bombicon.shp` | 725f690aaa 2014-03-20 Paul Chote - Add posters for TD airstrike and nuke. | download |
| `mods/cnc/bits/bti1.shp` | 365e7a9584 2015-04-28 reaperrr - Fix blue tiberium not appearing in legacy editor (TD) | download |
| `mods/cnc/bits/c17icnh.shp` | 008b0cbab5 2013-03-29 Matthias Mailänder - Added @Nyerguds TD Deluxe Cameo Pack | download |
| `mods/cnc/bits/camera.shp` | 88430f3a33 2015-11-17 reaperrr - Fix mpspawn, waypoint and camera art | download |
| `mods/cnc/bits/chan.shp` | 34a228406f 2015-08-03 Matthias Mailänder - fix shadow on frame 25 | download |
| `mods/cnc/bits/civcapt1.aud` | 1b19ac09aa 2014-07-27 reaperrr - Adds "Civilian building captured" notification to TD. | download |
| `mods/cnc/bits/cliffsl1.sno` | 4f0e317371 2017-03-10 Mustafa Alperen Seki - Add Nooze's 1 Tile Cliffs to TD | download |
| `mods/cnc/bits/cliffsl1.tem` | 4f0e317371 2017-03-10 Mustafa Alperen Seki - Add Nooze's 1 Tile Cliffs to TD | download |
| `mods/cnc/bits/cliffsl1.win` | 4f0e317371 2017-03-10 Mustafa Alperen Seki - Add Nooze's 1 Tile Cliffs to TD | download |
| `mods/cnc/bits/cliffsl2.sno` | 4f0e317371 2017-03-10 Mustafa Alperen Seki - Add Nooze's 1 Tile Cliffs to TD | download |
| `mods/cnc/bits/cliffsl2.tem` | 4f0e317371 2017-03-10 Mustafa Alperen Seki - Add Nooze's 1 Tile Cliffs to TD | download |
| `mods/cnc/bits/cliffsl2.win` | 4f0e317371 2017-03-10 Mustafa Alperen Seki - Add Nooze's 1 Tile Cliffs to TD | download |
| `mods/cnc/bits/cliffsl3.sno` | 4f0e317371 2017-03-10 Mustafa Alperen Seki - Add Nooze's 1 Tile Cliffs to TD | download |
| `mods/cnc/bits/cliffsl3.tem` | 4f0e317371 2017-03-10 Mustafa Alperen Seki - Add Nooze's 1 Tile Cliffs to TD | download |
| `mods/cnc/bits/cliffsl3.win` | 4f0e317371 2017-03-10 Mustafa Alperen Seki - Add Nooze's 1 Tile Cliffs to TD | download |
| `mods/cnc/bits/cliffsl4.sno` | 4f0e317371 2017-03-10 Mustafa Alperen Seki - Add Nooze's 1 Tile Cliffs to TD | download |
| `mods/cnc/bits/cliffsl4.tem` | 4f0e317371 2017-03-10 Mustafa Alperen Seki - Add Nooze's 1 Tile Cliffs to TD | download |
| `mods/cnc/bits/cliffsl4.win` | 4f0e317371 2017-03-10 Mustafa Alperen Seki - Add Nooze's 1 Tile Cliffs to TD | download |
| `mods/cnc/bits/cloakcrate.shp` | 9657d697fc 2011-06-30 Paul Chote - Add dino/vice icons, dino voices, crate effects from Sole Survivor | download |
| `mods/cnc/bits/cycl.shp` | a20d1c95c8 2012-07-01 Matthias Mailänder - noticable SHP fixes from C&C 1.06c rev3 | download |
| `mods/cnc/bits/desert/b06.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/cnc/bits/desert/br10.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/cnc/bits/desert/br5.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/cnc/bits/desert/cliffsl1.des` | ab6b7c8551 2017-03-10 Mustafa Alperen Seki - Add Nooze's 1 Tile Cliffs to RA | download |
| `mods/cnc/bits/desert/cliffsl2.des` | ab6b7c8551 2017-03-10 Mustafa Alperen Seki - Add Nooze's 1 Tile Cliffs to RA | download |
| `mods/cnc/bits/desert/cliffsl3.des` | ab6b7c8551 2017-03-10 Mustafa Alperen Seki - Add Nooze's 1 Tile Cliffs to RA | download |
| `mods/cnc/bits/desert/cliffsl4.des` | ab6b7c8551 2017-03-10 Mustafa Alperen Seki - Add Nooze's 1 Tile Cliffs to RA | download |
| `mods/cnc/bits/desert/p08.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/cnc/bits/desert/p09.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/cnc/bits/desert/p10.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/cnc/bits/dinoatk1.aud` | 9657d697fc 2011-06-30 Paul Chote - Add dino/vice icons, dino voices, crate effects from Sole Survivor | download |
| `mods/cnc/bits/dinodie1.aud` | 9657d697fc 2011-06-30 Paul Chote - Add dino/vice icons, dino voices, crate effects from Sole Survivor | download |
| `mods/cnc/bits/dinomout.aud` | 9657d697fc 2011-06-30 Paul Chote - Add dino/vice icons, dino voices, crate effects from Sole Survivor | download |
| `mods/cnc/bits/dinoyes.aud` | 9657d697fc 2011-06-30 Paul Chote - Add dino/vice icons, dino voices, crate effects from Sole Survivor | download |
| `mods/cnc/bits/enemya.aud` | 615ac2504a 2013-08-19 Matthias Mailänder - added "enemy planes approaching" EVA notification from demo.mix | download |
| `mods/cnc/bits/eye.shp` | 8cea309ec6 2012-07-02 Matthias Mailänder - more SHP fixes by Nyerguds | download |
| `mods/cnc/bits/eyemake.shp` | 8cea309ec6 2012-07-02 Matthias Mailänder - more SHP fixes by Nyerguds | download |
| `mods/cnc/bits/fact.shp` | a20d1c95c8 2012-07-01 Matthias Mailänder - noticable SHP fixes from C&C 1.06c rev3 | download |
| `mods/cnc/bits/facticnh.shp` | 008b0cbab5 2013-03-29 Matthias Mailänder - Added @Nyerguds TD Deluxe Cameo Pack | download |
| `mods/cnc/bits/fb4.shp` | 2593bc0a72 2013-04-01 psydev - added rocket trail, msam icon, blue-tree.shp | download |
| `mods/cnc/bits/fullshroud.shp` | 8f046c7c5a 2014-06-20 Paul Chote - Support split sprites for fog/shroud. | download |
| `mods/cnc/bits/gun5.aud` | 6817332317 2011-06-24 Paul Chote - Rework apache weapon and firing offsets. Has duel machine guns with 10 shots, reloads all of its ammo every 8 seconds. Damage increased, and can now attack air. Fixes second half of #930. | download |
| `mods/cnc/bits/gunfire2.shp` | a1ac1973a7 2013-08-05 reaperrr - Add muzzle flashes to all cannons and the Advanced Guard Tower (which had a muzzle in the original CnC, too). | download |
| `mods/cnc/bits/hand.shp` | a20d1c95c8 2012-07-01 Matthias Mailänder - noticable SHP fixes from C&C 1.06c rev3 | download |
| `mods/cnc/bits/handmake.shp` | a20d1c95c8 2012-07-01 Matthias Mailänder - noticable SHP fixes from C&C 1.06c rev3 | download |
| `mods/cnc/bits/harvdump.shp` | 089cdf14ad 2011-07-14 Paul Chote - Remove the baked harvester animation from proc. Fixes #1009. | download |
| `mods/cnc/bits/harvlost.aud` | ae8c8e781f 2020-08-15 Orb - TD Balance Summer 2020 2 | download |
| `mods/cnc/bits/hclock.shp` | e787c46a81 2011-07-03 Paul Chote - Remove border from buildclock overlay. | download |
| `mods/cnc/bits/healcrate.shp` | 9657d697fc 2011-06-30 Paul Chote - Add dino/vice icons, dino voices, crate effects from Sole Survivor | download |
| `mods/cnc/bits/heli.shp` | 9886c1f6fe 2011-06-27 Paul Chote - Fix unremappable pixels in heli.shp (Fixes #965). | download |
| `mods/cnc/bits/ionicnh.tem` | a20d1c95c8 2012-07-01 Matthias Mailänder - noticable SHP fixes from C&C 1.06c rev3 | download |
| `mods/cnc/bits/jungle/b1.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/b2.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/b3.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/bib1.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/bib2.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/bib3.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/bridge1.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/bridge1d.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/bridge2.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/bridge2d.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/clear1.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/cliffsl1.jun` | 4f0e317371 2017-03-10 Mustafa Alperen Seki - Add Nooze's 1 Tile Cliffs to TD | download |
| `mods/cnc/bits/jungle/cliffsl2.jun` | 4f0e317371 2017-03-10 Mustafa Alperen Seki - Add Nooze's 1 Tile Cliffs to TD | download |
| `mods/cnc/bits/jungle/cliffsl3.jun` | 4f0e317371 2017-03-10 Mustafa Alperen Seki - Add Nooze's 1 Tile Cliffs to TD | download |
| `mods/cnc/bits/jungle/cliffsl4.jun` | 4f0e317371 2017-03-10 Mustafa Alperen Seki - Add Nooze's 1 Tile Cliffs to TD | download |
| `mods/cnc/bits/jungle/cr1.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/cr2.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/cr3.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/cr4.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/cr5.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/cr6.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/d01.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/d02.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/d03.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/d04.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/d05.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/d06.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/d07.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/d08.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/d09.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/d10.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/d11.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/d12.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/d13.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/d14.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/d15.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/d16.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/d17.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/d18.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/d19.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/d20.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/d21.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/d22.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/d23.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/d24.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/d25.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/d26.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/d27.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/d28.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/d29.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/d30.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/d31.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/d32.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/d33.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/d34.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/d35.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/d36.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/d37.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/d38.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/d39.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/d40.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/d41.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/d42.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/d43.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/falls1.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/falls2.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/ford1.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/ford2.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/jungle.pal` | 7dad013b31 2011-01-20 Paul Chote - Fix cnc. | download |
| `mods/cnc/bits/jungle/p01.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/p02.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/p03.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/p04.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/p07.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/p08.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/p13.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/p14.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/rv01.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/rv02.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/rv03.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/rv04.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/rv05.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/rv06.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/rv07.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/rv08.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/rv09.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/rv10.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/rv11.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/rv12.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/rv13.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/s01.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/s02.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/s03.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/s04.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/s05.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/s06.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/s07.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/s08.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/s09.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/s10.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/s11.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/s12.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/s13.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/s14.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/s15.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/s16.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/s17.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/s18.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/s19.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/s20.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/s21.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/s22.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/s23.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/s24.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/s25.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/s26.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/s27.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/s28.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/s29.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/s30.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/s31.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/s32.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/s33.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/s34.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/s35.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/s36.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/s37.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/s38.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/sc1.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/sc2.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/sc3.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/sc4.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/sc5.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/sc6.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/sh1.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/sh10.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/sh11.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/sh12.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/sh13.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/sh14.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/sh15.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/sh16.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/sh17.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/sh18.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/sh2.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/sh3.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/sh32.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/sh33.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/sh34.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/sh35.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/sh4.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/sh5.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/sh6.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/sh7.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/sh8.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/sh9.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/split2.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/split3.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/sr1.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/sr2.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/t01.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/t02.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/t03.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/t05.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/t06.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/t07.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/t08.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/t10.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/t11.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/t12.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/t13.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/t14.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/t15.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/t16.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/t17.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/tc01.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/tc02.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/tc03.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/tc04.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/tc05.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/ti1.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/ti10.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/ti11.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/ti12.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/ti2.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/ti3.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/ti4.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/ti5.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/ti6.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/ti7.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/ti8.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/ti9.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/v01.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/v02.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/v03.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/v04.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/v05.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/v06.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/v07.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/v08.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/v09.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/v10.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/v11.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/v12.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/v13.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/v14.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/v15.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/v16.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/v17.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/v18.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/w1.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/jungle/w2.jun` | 52e2b83069 2014-05-19 Matthias Mailänder - add the JUNGLE terrain to Tiberian Dawn | download |
| `mods/cnc/bits/levelup.shp` | 230625897a 2010-05-27 Chris Forbes - hack CrateEffect to have a custom offset; add levelup.shp; change > to >= in GainsExperience | download |
| `mods/cnc/bits/lsticnh.shp` | 666f9b4e47 2014-02-22 Matthias Mailänder - added an icon for the hovercraft | download |
| `mods/cnc/bits/mbFIX.des` | fc2d0510d8 2014-03-19 Kanar - Adds minibibs for bibless buildings in RA and C&C. | download |
| `mods/cnc/bits/mbFIX.sno` | fc2d0510d8 2014-03-19 Kanar - Adds minibibs for bibless buildings in RA and C&C. | download |
| `mods/cnc/bits/mbFIX.tem` | fc2d0510d8 2014-03-19 Kanar - Adds minibibs for bibless buildings in RA and C&C. | download |
| `mods/cnc/bits/mbGTWR.des` | fc2d0510d8 2014-03-19 Kanar - Adds minibibs for bibless buildings in RA and C&C. | download |
| `mods/cnc/bits/mbGTWR.sno` | fc2d0510d8 2014-03-19 Kanar - Adds minibibs for bibless buildings in RA and C&C. | download |
| `mods/cnc/bits/mbGTWR.tem` | fc2d0510d8 2014-03-19 Kanar - Adds minibibs for bibless buildings in RA and C&C. | download |
| `mods/cnc/bits/mbGUN.des` | fc2d0510d8 2014-03-19 Kanar - Adds minibibs for bibless buildings in RA and C&C. | download |
| `mods/cnc/bits/mbGUN.sno` | fc2d0510d8 2014-03-19 Kanar - Adds minibibs for bibless buildings in RA and C&C. | download |
| `mods/cnc/bits/mbGUN.tem` | fc2d0510d8 2014-03-19 Kanar - Adds minibibs for bibless buildings in RA and C&C. | download |
| `mods/cnc/bits/mbHOSP.des` | bf47543a0a 2014-06-06 Kanar - Adds/reworks some minibibs | download |
| `mods/cnc/bits/mbHOSP.sno` | bf47543a0a 2014-06-06 Kanar - Adds/reworks some minibibs | download |
| `mods/cnc/bits/mbHOSP.tem` | bf47543a0a 2014-06-06 Kanar - Adds/reworks some minibibs | download |
| `mods/cnc/bits/mbMISS.des` | fc2d0510d8 2014-03-19 Kanar - Adds minibibs for bibless buildings in RA and C&C. | download |
| `mods/cnc/bits/mbMISS.sno` | fc2d0510d8 2014-03-19 Kanar - Adds minibibs for bibless buildings in RA and C&C. | download |
| `mods/cnc/bits/mbMISS.tem` | fc2d0510d8 2014-03-19 Kanar - Adds minibibs for bibless buildings in RA and C&C. | download |
| `mods/cnc/bits/mbOBLI.des` | fc2d0510d8 2014-03-19 Kanar - Adds minibibs for bibless buildings in RA and C&C. | download |
| `mods/cnc/bits/mbOBLI.sno` | fc2d0510d8 2014-03-19 Kanar - Adds minibibs for bibless buildings in RA and C&C. | download |
| `mods/cnc/bits/mbOBLI.tem` | fc2d0510d8 2014-03-19 Kanar - Adds minibibs for bibless buildings in RA and C&C. | download |
| `mods/cnc/bits/mbSILO.des` | fc2d0510d8 2014-03-19 Kanar - Adds minibibs for bibless buildings in RA and C&C. | download |
| `mods/cnc/bits/mbSILO.sno` | fc2d0510d8 2014-03-19 Kanar - Adds minibibs for bibless buildings in RA and C&C. | download |
| `mods/cnc/bits/mbSILO.tem` | fc2d0510d8 2014-03-19 Kanar - Adds minibibs for bibless buildings in RA and C&C. | download |
| `mods/cnc/bits/mhqicnh.shp` | 008b0cbab5 2013-03-29 Matthias Mailänder - Added @Nyerguds TD Deluxe Cameo Pack | download |
| `mods/cnc/bits/missicnh.shp` | d9efe82b5a 2013-03-29 Matthias Mailänder - added prison/tech center icon and build-up animation | download |
| `mods/cnc/bits/missmake.shp` | d9efe82b5a 2013-03-29 Matthias Mailänder - added prison/tech center icon and build-up animation | download |
| `mods/cnc/bits/mlrs.shp` | 8cea309ec6 2012-07-02 Matthias Mailänder - more SHP fixes by Nyerguds | download |
| `mods/cnc/bits/mlrsicnh.tem` | 2593bc0a72 2013-04-01 psydev - added rocket trail, msam icon, blue-tree.shp | download |
| `mods/cnc/bits/moebius.shp` | 34a228406f 2015-08-03 Matthias Mailänder - fix shadow on frame 25 | download |
| `mods/cnc/bits/mouse2.shp` | cbd12f90ae 2010-03-07 Chris Forbes - fix cnc cursors | download |
| `mods/cnc/bits/mouse3.shp` | 61006ee73b 2010-07-23 Paul Chote - For now, use ra cursors to fill some of the gaps in cnc | download |
| `mods/cnc/bits/mouse4.shp` | 5019bb8b6e 2010-07-23 Paul Chote - Animated move cursors for cnc | download |
| `mods/cnc/bits/mouse5.shp` | 865f6ed0d5 2014-03-08 Paul Chote - Add a new TD guard cursor. | download |
| `mods/cnc/bits/mouse6.shp` | cf75240c7e 2014-03-08 Paul Chote - Add new TD goldwrench / capture cursors. | download |
| `mods/cnc/bits/mouse7.shp` | 059ba575ee 2014-03-08 Paul Chote - Add a new attackoutsiderange cursor to TD. | download |
| `mods/cnc/bits/mpspawn.shp` | 5bd9273a7d 2011-02-15 Chris Forbes - fix issues with mpspawn and waypoint sprites | download |
| `mods/cnc/bits/nobuild1.aud` | 8264c6c8dc 2011-01-08 Paul Chote - Play "Unable to build more" eva when the production exit is blocked. Also fixes #484. | download |
| `mods/cnc/bits/nuyell10.aud` | 98e48ae991 2014-05-09 reaperrr - Converts InfDeath from integer to string. | download |
| `mods/cnc/bits/nuyell11.aud` | 98e48ae991 2014-05-09 reaperrr - Converts InfDeath from integer to string. | download |
| `mods/cnc/bits/nuyell12.aud` | 98e48ae991 2014-05-09 reaperrr - Converts InfDeath from integer to string. | download |
| `mods/cnc/bits/nuyell6.aud` | 98e48ae991 2014-05-09 reaperrr - Converts InfDeath from integer to string. | download |
| `mods/cnc/bits/nuyell7.aud` | 98e48ae991 2014-05-09 reaperrr - Converts InfDeath from integer to string. | download |
| `mods/cnc/bits/pdigits.shp` | f616a527ee 2010-09-09 Paul Chote - Reorganise cnc mod dir. Exposes a bug in folder/package priority. | download |
| `mods/cnc/bits/pip-hazmat.shp` | e69fcd385b 2016-01-31 Zimmermann Gyula - Add art for HazMat decoration to TD | download |
| `mods/cnc/bits/pip-heal.shp` | 02010a6dd2 2016-02-02 Zimmermann Gyula - Add art for medkit decoration to TD. | download |
| `mods/cnc/bits/pips.shp` | 6c129eed3c 2011-07-24 Paul Chote - Split pip artwork into their own files and kill the hardcoded size/offset junk. Add a blue pip to support C&C blue tib. Fixes #836. | download |
| `mods/cnc/bits/poweroff.shp` | f0876828f4 2013-03-20 Matthias Mailänder - fixes #2791 don't crash on powerdown hotkey when playing cnc | download |
| `mods/cnc/bits/proctwr.shp` | 996a5407a9 2011-07-24 Paul Chote - Use proc tower lights as a fullness indicator (Feature #1035). | download |
| `mods/cnc/bits/pvice.shp` | fc10b79e1d 2011-06-30 Paul Chote - Capturable Biolab allows production of player-controlled viceroids. | download |
| `mods/cnc/bits/pylemake.shp` | 8cea309ec6 2012-07-02 Matthias Mailänder - more SHP fixes by Nyerguds | download |
| `mods/cnc/bits/radarcrate.shp` | 9657d697fc 2011-06-30 Paul Chote - Add dino/vice icons, dino voices, crate effects from Sole Survivor | download |
| `mods/cnc/bits/rank.shp` | a259a0aca0 2010-05-27 Chris Forbes - hack in rank overlay; fix last rank; add an extra (elite) rank | download |
| `mods/cnc/bits/rapticnh.shp` | 9657d697fc 2011-06-30 Paul Chote - Add dino/vice icons, dino voices, crate effects from Sole Survivor | download |
| `mods/cnc/bits/repair1.aud` | 6e1cb23f10 2011-01-30 Paul Chote - "Repairing" eva notification for ra and cnc. | download |
| `mods/cnc/bits/rtib1.des` | 739865062a 2014-07-03 reaperrr - Replaces glitchy blue tiberium art with TS-style remapable tiberium art. | download |
| `mods/cnc/bits/rtib1.tem` | 739865062a 2014-07-03 reaperrr - Replaces glitchy blue tiberium art with TS-style remapable tiberium art. | download |
| `mods/cnc/bits/rtib10.des` | 739865062a 2014-07-03 reaperrr - Replaces glitchy blue tiberium art with TS-style remapable tiberium art. | download |
| `mods/cnc/bits/rtib10.tem` | 739865062a 2014-07-03 reaperrr - Replaces glitchy blue tiberium art with TS-style remapable tiberium art. | download |
| `mods/cnc/bits/rtib11.des` | 739865062a 2014-07-03 reaperrr - Replaces glitchy blue tiberium art with TS-style remapable tiberium art. | download |
| `mods/cnc/bits/rtib11.tem` | 739865062a 2014-07-03 reaperrr - Replaces glitchy blue tiberium art with TS-style remapable tiberium art. | download |
| `mods/cnc/bits/rtib12.des` | 739865062a 2014-07-03 reaperrr - Replaces glitchy blue tiberium art with TS-style remapable tiberium art. | download |
| `mods/cnc/bits/rtib12.tem` | 739865062a 2014-07-03 reaperrr - Replaces glitchy blue tiberium art with TS-style remapable tiberium art. | download |
| `mods/cnc/bits/rtib2.des` | 739865062a 2014-07-03 reaperrr - Replaces glitchy blue tiberium art with TS-style remapable tiberium art. | download |
| `mods/cnc/bits/rtib2.tem` | 739865062a 2014-07-03 reaperrr - Replaces glitchy blue tiberium art with TS-style remapable tiberium art. | download |
| `mods/cnc/bits/rtib3.des` | 739865062a 2014-07-03 reaperrr - Replaces glitchy blue tiberium art with TS-style remapable tiberium art. | download |
| `mods/cnc/bits/rtib3.tem` | 739865062a 2014-07-03 reaperrr - Replaces glitchy blue tiberium art with TS-style remapable tiberium art. | download |
| `mods/cnc/bits/rtib4.des` | 739865062a 2014-07-03 reaperrr - Replaces glitchy blue tiberium art with TS-style remapable tiberium art. | download |
| `mods/cnc/bits/rtib4.tem` | 739865062a 2014-07-03 reaperrr - Replaces glitchy blue tiberium art with TS-style remapable tiberium art. | download |
| `mods/cnc/bits/rtib5.des` | 739865062a 2014-07-03 reaperrr - Replaces glitchy blue tiberium art with TS-style remapable tiberium art. | download |
| `mods/cnc/bits/rtib5.tem` | 739865062a 2014-07-03 reaperrr - Replaces glitchy blue tiberium art with TS-style remapable tiberium art. | download |
| `mods/cnc/bits/rtib6.des` | 739865062a 2014-07-03 reaperrr - Replaces glitchy blue tiberium art with TS-style remapable tiberium art. | download |
| `mods/cnc/bits/rtib6.tem` | 739865062a 2014-07-03 reaperrr - Replaces glitchy blue tiberium art with TS-style remapable tiberium art. | download |
| `mods/cnc/bits/rtib7.des` | 739865062a 2014-07-03 reaperrr - Replaces glitchy blue tiberium art with TS-style remapable tiberium art. | download |
| `mods/cnc/bits/rtib7.tem` | 739865062a 2014-07-03 reaperrr - Replaces glitchy blue tiberium art with TS-style remapable tiberium art. | download |
| `mods/cnc/bits/rtib8.des` | 739865062a 2014-07-03 reaperrr - Replaces glitchy blue tiberium art with TS-style remapable tiberium art. | download |
| `mods/cnc/bits/rtib8.tem` | 739865062a 2014-07-03 reaperrr - Replaces glitchy blue tiberium art with TS-style remapable tiberium art. | download |
| `mods/cnc/bits/rtib9.des` | 739865062a 2014-07-03 reaperrr - Replaces glitchy blue tiberium art with TS-style remapable tiberium art. | download |
| `mods/cnc/bits/rtib9.tem` | 739865062a 2014-07-03 reaperrr - Replaces glitchy blue tiberium art with TS-style remapable tiberium art. | download |
| `mods/cnc/bits/sandbag2.aud` | 76216b8dd9 2011-01-04 Paul Chote - Move damage sounds and shaking from Building/RenderBuilding into their own traits. Fix wall damage/death sounds under cnc. | download |
| `mods/cnc/bits/scrate.shp` | 13a101f11f 2021-03-21 reaperrr - Add a frame with shadow to both TD crate types | download |
| `mods/cnc/bits/silo.shp` | 8cea309ec6 2012-07-02 Matthias Mailänder - more SHP fixes by Nyerguds | download |
| `mods/cnc/bits/silomake.shp` | 8cea309ec6 2012-07-02 Matthias Mailänder - more SHP fixes by Nyerguds | download |
| `mods/cnc/bits/snow.mix` | 5c6a6ee350 2010-09-10 Paul Chote - Add snow.mix | download |
| `mods/cnc/bits/snow.pal` | 7dad013b31 2011-01-20 Paul Chote - Fix cnc. | download |
| `mods/cnc/bits/ss/armorcrate.shp` | 8ada3d34ec 2013-12-27 Curtis Shmyr - Initial support for individual unit upgrade crates | download |
| `mods/cnc/bits/ss/armorup.aud` | 8ada3d34ec 2013-12-27 Curtis Shmyr - Initial support for individual unit upgrade crates | download |
| `mods/cnc/bits/ss/firepowercrate.shp` | 8ada3d34ec 2013-12-27 Curtis Shmyr - Initial support for individual unit upgrade crates | download |
| `mods/cnc/bits/ss/firepowerup.aud` | 8ada3d34ec 2013-12-27 Curtis Shmyr - Initial support for individual unit upgrade crates | download |
| `mods/cnc/bits/ss/rangeup.aud` | 8ada3d34ec 2013-12-27 Curtis Shmyr - Initial support for individual unit upgrade crates | download |
| `mods/cnc/bits/ss/rateoffireup.aud` | 8ada3d34ec 2013-12-27 Curtis Shmyr - Initial support for individual unit upgrade crates | download |
| `mods/cnc/bits/ss/speedcrate.shp` | 8ada3d34ec 2013-12-27 Curtis Shmyr - Initial support for individual unit upgrade crates | download |
| `mods/cnc/bits/ss/speedup.aud` | 8ada3d34ec 2013-12-27 Curtis Shmyr - Initial support for individual unit upgrade crates | download |
| `mods/cnc/bits/stegicnh.shp` | 9657d697fc 2011-06-30 Paul Chote - Add dino/vice icons, dino voices, crate effects from Sole Survivor | download |
| `mods/cnc/bits/tmplicnh.shp` | 008b0cbab5 2013-03-29 Matthias Mailänder - Added @Nyerguds TD Deluxe Cameo Pack | download |
| `mods/cnc/bits/tone15.aud` | c2e08922f5 2014-09-30 s1ww - Normalizing CashTicks AUD for TD | download |
| `mods/cnc/bits/tone16.aud` | c2e08922f5 2014-09-30 s1ww - Normalizing CashTicks AUD for TD | download |
| `mods/cnc/bits/toss1.aud` | e4bc965776 2013-10-27 Paul Chote - Add grenade toss sound (from C&C demo). | download |
| `mods/cnc/bits/trexicnh.shp` | 9657d697fc 2011-06-30 Paul Chote - Add dino/vice icons, dino voices, crate effects from Sole Survivor | download |
| `mods/cnc/bits/tricicnh.shp` | 9657d697fc 2011-06-30 Paul Chote - Add dino/vice icons, dino voices, crate effects from Sole Survivor | download |
| `mods/cnc/bits/truck.shp` | ce671a2737 2017-05-03 andrew - TD Supply Truck | download |
| `mods/cnc/bits/truckicon.shp` | ce671a2737 2017-05-03 andrew - TD Supply Truck | download |
| `mods/cnc/bits/tunnel1.tem` | 4f0e317371 2017-03-10 Mustafa Alperen Seki - Add Nooze's 1 Tile Cliffs to TD | download |
| `mods/cnc/bits/tunnel2.tem` | 4f0e317371 2017-03-10 Mustafa Alperen Seki - Add Nooze's 1 Tile Cliffs to TD | download |
| `mods/cnc/bits/tunnel3.tem` | 4f0e317371 2017-03-10 Mustafa Alperen Seki - Add Nooze's 1 Tile Cliffs to TD | download |
| `mods/cnc/bits/tunnel4.tem` | 4f0e317371 2017-03-10 Mustafa Alperen Seki - Add Nooze's 1 Tile Cliffs to TD | download |
| `mods/cnc/bits/v19.shp` | 5e0817fc37 2011-06-27 Daniel Hernandez - Remappable oil derricks. Fixes #502. | download |
| `mods/cnc/bits/viceicnh.shp` | 9657d697fc 2011-06-30 Paul Chote - Add dino/vice icons, dino voices, crate effects from Sole Survivor | download |
| `mods/cnc/bits/wake.shp` | 9bca5f5131 2014-10-17 reaperrr - Fix RenderGunboat wake animation | download |
| `mods/cnc/bits/waypoint.shp` | 5bd9273a7d 2011-02-15 Chris Forbes - fix issues with mpspawn and waypoint sprites | download |
| `mods/cnc/bits/wcrate.shp` | 13a101f11f 2021-03-21 reaperrr - Add a frame with shadow to both TD crate types | download |
| `mods/cnc/bits/xcrate.shp` | e9e2e353f7 2013-12-15 Paul Chote - Change crate graphics for christmas. Closes #4296. | download |
| `mods/cnc/icon-2x.png` | fb7c781a66 2020-02-15 Paul Chote - Add multi-resolution mod icon support. | download |
| `mods/cnc/icon-3x.png` | fb7c781a66 2020-02-15 Paul Chote - Add multi-resolution mod icon support. | download |
| `mods/cnc/icon.png` | 4d982f00e4 2017-01-20 Paul Chote - Add 32px icons for default mods. | download |
| `mods/cnc/maps/gdi06/rain-ambient.aud` | f6ae27b4b7 2015-07-05 Matthias Mailänder - add thunderstorm effects to fort lonestar | ship |
| `mods/cnc/uibits/chrome-2x.png` | 8c2a2d2cb8 2020-01-08 Paul Chote - Rework TD/modcontent chrome in preparation for UI scaling. | download |
| `mods/cnc/uibits/chrome-3x.png` | 8c2a2d2cb8 2020-01-08 Paul Chote - Rework TD/modcontent chrome in preparation for UI scaling. | download |
| `mods/cnc/uibits/chrome.png` | a65021b550 2011-05-15 Paul Chote - Merge most of the new ui into a single texture | download |
| `mods/cnc/uibits/encyclopedia/a10.png` | 9bcf1b3295 2024-10-30 Paul Chote - Overhaul TD EVA Database. | download |
| `mods/cnc/uibits/encyclopedia/afld.png` | 9bcf1b3295 2024-10-30 Paul Chote - Overhaul TD EVA Database. | download |
| `mods/cnc/uibits/encyclopedia/apc.png` | 9bcf1b3295 2024-10-30 Paul Chote - Overhaul TD EVA Database. | download |
| `mods/cnc/uibits/encyclopedia/arty.png` | 9bcf1b3295 2024-10-30 Paul Chote - Overhaul TD EVA Database. | download |
| `mods/cnc/uibits/encyclopedia/atwr.png` | 9bcf1b3295 2024-10-30 Paul Chote - Overhaul TD EVA Database. | download |
| `mods/cnc/uibits/encyclopedia/bggy.png` | 9bcf1b3295 2024-10-30 Paul Chote - Overhaul TD EVA Database. | download |
| `mods/cnc/uibits/encyclopedia/bike.png` | 9bcf1b3295 2024-10-30 Paul Chote - Overhaul TD EVA Database. | download |
| `mods/cnc/uibits/encyclopedia/boat.png` | 9bcf1b3295 2024-10-30 Paul Chote - Overhaul TD EVA Database. | download |
| `mods/cnc/uibits/encyclopedia/brik.png` | 9bcf1b3295 2024-10-30 Paul Chote - Overhaul TD EVA Database. | download |
| `mods/cnc/uibits/encyclopedia/c17.png` | 9bcf1b3295 2024-10-30 Paul Chote - Overhaul TD EVA Database. | download |
| `mods/cnc/uibits/encyclopedia/cycl.png` | 9bcf1b3295 2024-10-30 Paul Chote - Overhaul TD EVA Database. | download |
| `mods/cnc/uibits/encyclopedia/default.png` | 9bcf1b3295 2024-10-30 Paul Chote - Overhaul TD EVA Database. | download |
| `mods/cnc/uibits/encyclopedia/e1.png` | 9bcf1b3295 2024-10-30 Paul Chote - Overhaul TD EVA Database. | download |
| `mods/cnc/uibits/encyclopedia/e2.png` | 9bcf1b3295 2024-10-30 Paul Chote - Overhaul TD EVA Database. | download |
| `mods/cnc/uibits/encyclopedia/e3.png` | 9bcf1b3295 2024-10-30 Paul Chote - Overhaul TD EVA Database. | download |
| `mods/cnc/uibits/encyclopedia/e4.png` | 9bcf1b3295 2024-10-30 Paul Chote - Overhaul TD EVA Database. | download |
| `mods/cnc/uibits/encyclopedia/e5.png` | 9bcf1b3295 2024-10-30 Paul Chote - Overhaul TD EVA Database. | download |
| `mods/cnc/uibits/encyclopedia/e6.png` | 9bcf1b3295 2024-10-30 Paul Chote - Overhaul TD EVA Database. | download |
| `mods/cnc/uibits/encyclopedia/eye.png` | 9bcf1b3295 2024-10-30 Paul Chote - Overhaul TD EVA Database. | download |
| `mods/cnc/uibits/encyclopedia/fact.png` | 9bcf1b3295 2024-10-30 Paul Chote - Overhaul TD EVA Database. | download |
| `mods/cnc/uibits/encyclopedia/fix.png` | 9bcf1b3295 2024-10-30 Paul Chote - Overhaul TD EVA Database. | download |
| `mods/cnc/uibits/encyclopedia/ftnk.png` | 9bcf1b3295 2024-10-30 Paul Chote - Overhaul TD EVA Database. | download |
| `mods/cnc/uibits/encyclopedia/gtwr.png` | 9bcf1b3295 2024-10-30 Paul Chote - Overhaul TD EVA Database. | download |
| `mods/cnc/uibits/encyclopedia/gun.png` | 9bcf1b3295 2024-10-30 Paul Chote - Overhaul TD EVA Database. | download |
| `mods/cnc/uibits/encyclopedia/hand.png` | 9bcf1b3295 2024-10-30 Paul Chote - Overhaul TD EVA Database. | download |
| `mods/cnc/uibits/encyclopedia/harv.png` | 9bcf1b3295 2024-10-30 Paul Chote - Overhaul TD EVA Database. | download |
| `mods/cnc/uibits/encyclopedia/heli.png` | 9bcf1b3295 2024-10-30 Paul Chote - Overhaul TD EVA Database. | download |
| `mods/cnc/uibits/encyclopedia/hpad.png` | 9bcf1b3295 2024-10-30 Paul Chote - Overhaul TD EVA Database. | download |
| `mods/cnc/uibits/encyclopedia/hq.png` | 9bcf1b3295 2024-10-30 Paul Chote - Overhaul TD EVA Database. | download |
| `mods/cnc/uibits/encyclopedia/htnk.png` | 9bcf1b3295 2024-10-30 Paul Chote - Overhaul TD EVA Database. | download |
| `mods/cnc/uibits/encyclopedia/jeep.png` | 9bcf1b3295 2024-10-30 Paul Chote - Overhaul TD EVA Database. | download |
| `mods/cnc/uibits/encyclopedia/lst.png` | 9bcf1b3295 2024-10-30 Paul Chote - Overhaul TD EVA Database. | download |
| `mods/cnc/uibits/encyclopedia/ltnk.png` | 9bcf1b3295 2024-10-30 Paul Chote - Overhaul TD EVA Database. | download |
| `mods/cnc/uibits/encyclopedia/mcv.png` | 9bcf1b3295 2024-10-30 Paul Chote - Overhaul TD EVA Database. | download |
| `mods/cnc/uibits/encyclopedia/mhq.png` | 9bcf1b3295 2024-10-30 Paul Chote - Overhaul TD EVA Database. | download |
| `mods/cnc/uibits/encyclopedia/mlrs.png` | 9bcf1b3295 2024-10-30 Paul Chote - Overhaul TD EVA Database. | download |
| `mods/cnc/uibits/encyclopedia/msam.png` | 9bcf1b3295 2024-10-30 Paul Chote - Overhaul TD EVA Database. | download |
| `mods/cnc/uibits/encyclopedia/mtnk.png` | 9bcf1b3295 2024-10-30 Paul Chote - Overhaul TD EVA Database. | download |
| `mods/cnc/uibits/encyclopedia/nuk2.png` | 9bcf1b3295 2024-10-30 Paul Chote - Overhaul TD EVA Database. | download |
| `mods/cnc/uibits/encyclopedia/nuke.png` | 9bcf1b3295 2024-10-30 Paul Chote - Overhaul TD EVA Database. | download |
| `mods/cnc/uibits/encyclopedia/obli.png` | 9bcf1b3295 2024-10-30 Paul Chote - Overhaul TD EVA Database. | download |
| `mods/cnc/uibits/encyclopedia/orca.png` | 9bcf1b3295 2024-10-30 Paul Chote - Overhaul TD EVA Database. | download |
| `mods/cnc/uibits/encyclopedia/proc.png` | 9bcf1b3295 2024-10-30 Paul Chote - Overhaul TD EVA Database. | download |
| `mods/cnc/uibits/encyclopedia/pvice.png` | 9bcf1b3295 2024-10-30 Paul Chote - Overhaul TD EVA Database. | download |
| `mods/cnc/uibits/encyclopedia/pyle.png` | 9bcf1b3295 2024-10-30 Paul Chote - Overhaul TD EVA Database. | download |
| `mods/cnc/uibits/encyclopedia/rmbo.png` | 9bcf1b3295 2024-10-30 Paul Chote - Overhaul TD EVA Database. | download |
| `mods/cnc/uibits/encyclopedia/sam.png` | 9bcf1b3295 2024-10-30 Paul Chote - Overhaul TD EVA Database. | download |
| `mods/cnc/uibits/encyclopedia/sbag.png` | 9bcf1b3295 2024-10-30 Paul Chote - Overhaul TD EVA Database. | download |
| `mods/cnc/uibits/encyclopedia/scanlines.png` | 9bcf1b3295 2024-10-30 Paul Chote - Overhaul TD EVA Database. | download |
| `mods/cnc/uibits/encyclopedia/silo.png` | 9bcf1b3295 2024-10-30 Paul Chote - Overhaul TD EVA Database. | download |
| `mods/cnc/uibits/encyclopedia/stnk.png` | 9bcf1b3295 2024-10-30 Paul Chote - Overhaul TD EVA Database. | download |
| `mods/cnc/uibits/encyclopedia/tmpl.png` | 9bcf1b3295 2024-10-30 Paul Chote - Overhaul TD EVA Database. | download |
| `mods/cnc/uibits/encyclopedia/tran.png` | 9bcf1b3295 2024-10-30 Paul Chote - Overhaul TD EVA Database. | download |
| `mods/cnc/uibits/encyclopedia/truck.png` | 9bcf1b3295 2024-10-30 Paul Chote - Overhaul TD EVA Database. | download |
| `mods/cnc/uibits/encyclopedia/weap.png` | 9bcf1b3295 2024-10-30 Paul Chote - Overhaul TD EVA Database. | download |
| `mods/common-content/chrome-2x.png` | 8c2a2d2cb8 2020-01-08 Paul Chote - Rework TD/modcontent chrome in preparation for UI scaling. | download original, ship blanked copy |
| `mods/common-content/chrome-3x.png` | 8c2a2d2cb8 2020-01-08 Paul Chote - Rework TD/modcontent chrome in preparation for UI scaling. | download original, ship blanked copy |
| `mods/common-content/chrome.png` | c720cb0b41 2014-03-23 Paul Chote - Extract mod selector into its own mod. | download original, ship blanked copy |
| `mods/common-content/cursor.png` | f36d0cc214 2019-12-15 Paul Chote - Fix ModContent cursor. | ship |
| `mods/common/FreeSans.ttf` | 31a933d5ac 2010-02-20 pchote - Commit what i have (doesn't work) for debug | ship |
| `mods/common/FreeSansBold.ttf` | 6e91c8015c 2010-02-20 Chris Forbes - hack hack hack... we have working text rendering | ship |
| `mods/ra/ZoodRangmah.ttf` | 9e11932ade 2010-04-13 Chris Forbes - hacked victory conditions | download |
| `mods/ra/bits/afldidle.shp` | 4e7f04da5f 2013-05-20 reaperrr - airfield gfx improvements | download |
| `mods/ra/bits/allyrepair.shp` | be9948426c 2011-09-10 Curtis S - 1078: Added ally repairing for CNC and RA | download |
| `mods/ra/bits/ammobox1.shp` | 4cebe6b9dc 2013-07-08 Matthias Mailänder - split ammobox into 3 single SHPs because of editor limitations | download |
| `mods/ra/bits/ammobox2.shp` | 4cebe6b9dc 2013-07-08 Matthias Mailänder - split ammobox into 3 single SHPs because of editor limitations | download |
| `mods/ra/bits/ammobox3.shp` | 4cebe6b9dc 2013-07-08 Matthias Mailänder - split ammobox into 3 single SHPs because of editor limitations | download |
| `mods/ra/bits/anticon.shp` | f638bf4156 2013-06-24 Matthias Mailänder - added Giant Ant from Counterstrike | download |
| `mods/ra/bits/apwrdead.shp` | 77d17ce761 2013-04-19 Matthias Mailänder - added building husks for RA | download |
| `mods/ra/bits/asianhut.shp` | 2d1a4404bc 2013-07-08 Matthias Mailänder - added an Asian hut by @JOoGraphics | download |
| `mods/ra/bits/ateficon.shp` | 1986b31bcb 2015-01-23 ScottNZ - Add fake atek (atef), mslo (mslf) and pdox (pdof) structures to ra. Thanks to MicroBit for the icons. | download |
| `mods/ra/bits/attackmove.shp` | 8063e25f3b 2014-03-08 Paul Chote - Add a new RA AttackMove cursor by @pizzaoverhead. | download |
| `mods/ra/bits/bctrinit.aud` | 0880c1241d 2013-03-22 Matthias Mailänder - fixes #2190 added announcer voice upon game start | download |
| `mods/ra/bits/beaconclock.shp` | e1501d5b7c 2014-05-31 Paul Chote - Add clock overlay to support power beacons. | download |
| `mods/ra/bits/bubbles.shp` | 846371cf3e 2010-09-23 Chris Forbes - homing torps, bubble trail | download |
| `mods/ra/bits/c11.shp` | 60e9d3f660 2013-01-27 Scott_NZ - Add c3 civilian, made by JOo | download |
| `mods/ra/bits/camera.shp` | 7f9f29c8a2 2015-03-01 Matthias Mailänder - add a camera icon | download |
| `mods/ra/bits/chan.shp` | 34a228406f 2015-08-03 Matthias Mailänder - fix shadow on frame 25 | download |
| `mods/ra/bits/cliffsl1.sno` | ab6b7c8551 2017-03-10 Mustafa Alperen Seki - Add Nooze's 1 Tile Cliffs to RA | download |
| `mods/ra/bits/cliffsl1.tem` | ab6b7c8551 2017-03-10 Mustafa Alperen Seki - Add Nooze's 1 Tile Cliffs to RA | download |
| `mods/ra/bits/cliffsl2.sno` | ab6b7c8551 2017-03-10 Mustafa Alperen Seki - Add Nooze's 1 Tile Cliffs to RA | download |
| `mods/ra/bits/cliffsl2.tem` | ab6b7c8551 2017-03-10 Mustafa Alperen Seki - Add Nooze's 1 Tile Cliffs to RA | download |
| `mods/ra/bits/cliffsl3.sno` | ab6b7c8551 2017-03-10 Mustafa Alperen Seki - Add Nooze's 1 Tile Cliffs to RA | download |
| `mods/ra/bits/cliffsl3.tem` | ab6b7c8551 2017-03-10 Mustafa Alperen Seki - Add Nooze's 1 Tile Cliffs to RA | download |
| `mods/ra/bits/cliffsl4.sno` | ab6b7c8551 2017-03-10 Mustafa Alperen Seki - Add Nooze's 1 Tile Cliffs to RA | download |
| `mods/ra/bits/cliffsl4.tem` | ab6b7c8551 2017-03-10 Mustafa Alperen Seki - Add Nooze's 1 Tile Cliffs to RA | download |
| `mods/ra/bits/cliffsw1.sno` | ab6b7c8551 2017-03-10 Mustafa Alperen Seki - Add Nooze's 1 Tile Cliffs to RA | download |
| `mods/ra/bits/cliffsw1.tem` | ab6b7c8551 2017-03-10 Mustafa Alperen Seki - Add Nooze's 1 Tile Cliffs to RA | download |
| `mods/ra/bits/cliffsw2.sno` | ab6b7c8551 2017-03-10 Mustafa Alperen Seki - Add Nooze's 1 Tile Cliffs to RA | download |
| `mods/ra/bits/cliffsw2.tem` | ab6b7c8551 2017-03-10 Mustafa Alperen Seki - Add Nooze's 1 Tile Cliffs to RA | download |
| `mods/ra/bits/cliffsw3.sno` | ab6b7c8551 2017-03-10 Mustafa Alperen Seki - Add Nooze's 1 Tile Cliffs to RA | download |
| `mods/ra/bits/cliffsw3.tem` | ab6b7c8551 2017-03-10 Mustafa Alperen Seki - Add Nooze's 1 Tile Cliffs to RA | download |
| `mods/ra/bits/cliffsw4.sno` | ab6b7c8551 2017-03-10 Mustafa Alperen Seki - Add Nooze's 1 Tile Cliffs to RA | download |
| `mods/ra/bits/cliffsw4.tem` | ab6b7c8551 2017-03-10 Mustafa Alperen Seki - Add Nooze's 1 Tile Cliffs to RA | download |
| `mods/ra/bits/cr4.sno` | 107c37f33d 2012-12-03 Matthias Mailänder - RA: Snow craters fix by MrFlibble | download |
| `mods/ra/bits/cr5.sno` | 107c37f33d 2012-12-03 Matthias Mailänder - RA: Snow craters fix by MrFlibble | download |
| `mods/ra/bits/ctflag.shp` | 91d79d4f42 2014-04-25 MicroGraphics - Added "Capture-the" Flag | download |
| `mods/ra/bits/deca.tem` | 9ce8c90d25 2012-07-28 Matthias Mailänder - added Harissons high-quality temperate terrain expansion | download |
| `mods/ra/bits/decb.tem` | 9ce8c90d25 2012-07-28 Matthias Mailänder - added Harissons high-quality temperate terrain expansion | download |
| `mods/ra/bits/decc.tem` | 9ce8c90d25 2012-07-28 Matthias Mailänder - added Harissons high-quality temperate terrain expansion | download |
| `mods/ra/bits/decd.tem` | 9ce8c90d25 2012-07-28 Matthias Mailänder - added Harissons high-quality temperate terrain expansion | download |
| `mods/ra/bits/dece.tem` | 9ce8c90d25 2012-07-28 Matthias Mailänder - added Harissons high-quality temperate terrain expansion | download |
| `mods/ra/bits/decf.tem` | 9ce8c90d25 2012-07-28 Matthias Mailänder - added Harissons high-quality temperate terrain expansion | download |
| `mods/ra/bits/decg.tem` | 9ce8c90d25 2012-07-28 Matthias Mailänder - added Harissons high-quality temperate terrain expansion | download |
| `mods/ra/bits/dech.tem` | 9ce8c90d25 2012-07-28 Matthias Mailänder - added Harissons high-quality temperate terrain expansion | download |
| `mods/ra/bits/desert/b01.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/b02.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/b03.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/b04.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/b05.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/b06.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/br01.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/br02.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/br03.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/br04.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/br05.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/br06.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/br07.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/br08.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/br09.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/br10.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/br1a.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/br1b.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/br1c.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/br1x.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/br2a.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/br2b.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/br2c.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/br2x.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/br3a.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/br3b.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/br3c.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/br3d.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/br3e.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/br3f.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/bridge1.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/bridge1d.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/bridge1h.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/bridge1x.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/bridge2.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/bridge2d.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/bridge2h.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/bridge2x.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/clear1a.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/cliffsl1.des` | ab6b7c8551 2017-03-10 Mustafa Alperen Seki - Add Nooze's 1 Tile Cliffs to RA | download |
| `mods/ra/bits/desert/cliffsl2.des` | ab6b7c8551 2017-03-10 Mustafa Alperen Seki - Add Nooze's 1 Tile Cliffs to RA | download |
| `mods/ra/bits/desert/cliffsl3.des` | ab6b7c8551 2017-03-10 Mustafa Alperen Seki - Add Nooze's 1 Tile Cliffs to RA | download |
| `mods/ra/bits/desert/cliffsl4.des` | ab6b7c8551 2017-03-10 Mustafa Alperen Seki - Add Nooze's 1 Tile Cliffs to RA | download |
| `mods/ra/bits/desert/cliffsw1.des` | ab6b7c8551 2017-03-10 Mustafa Alperen Seki - Add Nooze's 1 Tile Cliffs to RA | download |
| `mods/ra/bits/desert/cliffsw2.des` | ab6b7c8551 2017-03-10 Mustafa Alperen Seki - Add Nooze's 1 Tile Cliffs to RA | download |
| `mods/ra/bits/desert/cliffsw3.des` | ab6b7c8551 2017-03-10 Mustafa Alperen Seki - Add Nooze's 1 Tile Cliffs to RA | download |
| `mods/ra/bits/desert/cliffsw4.des` | ab6b7c8551 2017-03-10 Mustafa Alperen Seki - Add Nooze's 1 Tile Cliffs to RA | download |
| `mods/ra/bits/desert/d44.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/d45.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/dt01.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/dt02.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/dt03.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/dt04.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/f01.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/f02.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/f03.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/f04.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/f05.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/f06.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/falls3.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/falls4.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/gmine.des` | 18663323aa 2014-10-26 Matthias Mailänder - add the gem mine by @MicroJOo | download |
| `mods/ra/bits/desert/hbox.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/hboxmake.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/mbAGUN.des` | fc2d0510d8 2014-03-19 Kanar - Adds minibibs for bibless buildings in RA and C&C. | download |
| `mods/ra/bits/desert/mbFIX.des` | fc2d0510d8 2014-03-19 Kanar - Adds minibibs for bibless buildings in RA and C&C. | download |
| `mods/ra/bits/desert/mbFTUR.des` | fc2d0510d8 2014-03-19 Kanar - Adds minibibs for bibless buildings in RA and C&C. | download |
| `mods/ra/bits/desert/mbGAP.des` | fc2d0510d8 2014-03-19 Kanar - Adds minibibs for bibless buildings in RA and C&C. | download |
| `mods/ra/bits/desert/mbGUN.des` | fc2d0510d8 2014-03-19 Kanar - Adds minibibs for bibless buildings in RA and C&C. | download |
| `mods/ra/bits/desert/mbHOSP.des` | bf47543a0a 2014-06-06 Kanar - Adds/reworks some minibibs | download |
| `mods/ra/bits/desert/mbIRON.des` | fc2d0510d8 2014-03-19 Kanar - Adds minibibs for bibless buildings in RA and C&C. | download |
| `mods/ra/bits/desert/mbPBOX.des` | fc2d0510d8 2014-03-19 Kanar - Adds minibibs for bibless buildings in RA and C&C. | download |
| `mods/ra/bits/desert/mbPDOX.des` | fc2d0510d8 2014-03-19 Kanar - Adds minibibs for bibless buildings in RA and C&C. | download |
| `mods/ra/bits/desert/mbSAM.des` | fc2d0510d8 2014-03-19 Kanar - Adds minibibs for bibless buildings in RA and C&C. | download |
| `mods/ra/bits/desert/mbSILO.des` | fc2d0510d8 2014-03-19 Kanar - Adds minibibs for bibless buildings in RA and C&C. | download |
| `mods/ra/bits/desert/mbTSLA.des` | fc2d0510d8 2014-03-19 Kanar - Adds minibibs for bibless buildings in RA and C&C. | download |
| `mods/ra/bits/desert/mine.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/mslo.des` | c11a6c13a6 2014-03-05 Matthias Mailänder - added a desert variation of TENT and MSLO | download |
| `mods/ra/bits/desert/mslomake.des` | c11a6c13a6 2014-03-05 Matthias Mailänder - added a desert variation of TENT and MSLO | download |
| `mods/ra/bits/desert/p08.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/p09.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/p10.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/railmine.des` | 8cba84d47d 2014-09-25 s1ww - Reworked railmines for each tileset | download |
| `mods/ra/bits/desert/rc01.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/rc02.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/rc03.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/rc04.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/rv26.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/rv27.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/rvm01.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/rvm02.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/rvm03.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/rvm04.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/sbrd1.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/sbrd1d.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/sbrd1h.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/sbrd1x.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/sbrd2.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/sbrd2d.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/sbrd2h.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/sbrd2x.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/sh64.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/sh65.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/tc01.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/tent.des` | c11a6c13a6 2014-03-05 Matthias Mailänder - added a desert variation of TENT and MSLO | download |
| `mods/ra/bits/desert/tentmake.des` | 9ce25bd400 2014-03-02 Matthias Mailänder - added a snow variation of TENT by Kilkakon | download |
| `mods/ra/bits/desert/v19.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/w1.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/wc01.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/wc02.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/wc03.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/wc04.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/wc05.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/wc06.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/wc07.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/wc08.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/wc09.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/wc10.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/wc11.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/wc12.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/wc13.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/wc14.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/wc15.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/wc16.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/wc17.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/wc18.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/wc19.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/wc20.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/wc21.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/wc22.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/wc23.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/wc24.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/wc25.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/wc26.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/wc27.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/wc28.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/wc29.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/wc30.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/wc31.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/wc32.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/wc33.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/wc34.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/wc35.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/wc36.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/wc37.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/wc38.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/wc39.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/wc40.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/wc41.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/wc42.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/wc43.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/wc44.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/wc45.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/desert/wc46.des` | c859fb1186 2012-10-21 Matthias Mailänder - fix the remaining issues with the ra desert terrain | download |
| `mods/ra/bits/e6.shp` | 1b0708eedc 2014-03-19 reaperrr - RA Engineer now uses team colors just like in C&C1. | download |
| `mods/ra/bits/einstein.shp` | 34a228406f 2015-08-03 Matthias Mailänder - fix shadow on frame 25 | download |
| `mods/ra/bits/enmydet.aud` | 873c47127a 2015-04-04 penev92 - Add "Enemy detected" notification to RA | download |
| `mods/ra/bits/fact.shp` | e1669ecd70 2013-05-20 reaperrr - construction yard with shadow | download |
| `mods/ra/bits/factdead.shp` | 77d17ce761 2013-04-19 Matthias Mailänder - added building husks for RA | download |
| `mods/ra/bits/factmake.shp` | e1669ecd70 2013-05-20 reaperrr - construction yard with shadow | download |
| `mods/ra/bits/fapwicon.shp` | f7be6d4d51 2016-03-23 Mustafa Alperen Seki - Added Fake Power Plants | download |
| `mods/ra/bits/fb3.shp` | cd6bfb1b9a 2012-04-30 Chris Forbes - add better flame projectile SHP | download |
| `mods/ra/bits/fb4.shp` | 80f142c8ea 2012-04-30 Chris Forbes - flamer that doesnt suck | download |
| `mods/ra/bits/fcommake.shp` | f2c630731e 2012-07-21 Matthias Mailänder - added some files from content.open-ra.org, fixes #2182 | download |
| `mods/ra/bits/fixficon.shp` | b3b355dd6f 2016-05-30 D'Arcy Rush - Add fake service depot. | download |
| `mods/ra/bits/fjord1.tem` | 9ce8c90d25 2012-07-28 Matthias Mailänder - added Harissons high-quality temperate terrain expansion | download |
| `mods/ra/bits/fjord2.tem` | 9ce8c90d25 2012-07-28 Matthias Mailänder - added Harissons high-quality temperate terrain expansion | download |
| `mods/ra/bits/fpwricon.shp` | f7be6d4d51 2016-03-23 Mustafa Alperen Seki - Added Fake Power Plants | download |
| `mods/ra/bits/ftrk.shp` | b8ed9d7da5 2010-09-09 Chris Forbes - (katzsmile) Added: Soviet Flak Truck with new SHP and Cameo | download |
| `mods/ra/bits/ftrkicon.shp` | b8ed9d7da5 2010-09-09 Chris Forbes - (katzsmile) Added: Soviet Flak Truck with new SHP and Cameo | download |
| `mods/ra/bits/gap.shp` | 2abb6421fe 2013-05-20 reaperrr - gap gen & mgg with shadows | download |
| `mods/ra/bits/gapmake.shp` | 2abb6421fe 2013-05-20 reaperrr - gap gen & mgg with shadows | download |
| `mods/ra/bits/gmine.sno` | 18663323aa 2014-10-26 Matthias Mailänder - add the gem mine by @MicroJOo | download |
| `mods/ra/bits/gmine.tem` | 18663323aa 2014-10-26 Matthias Mailänder - add the gem mine by @MicroJOo | download |
| `mods/ra/bits/gpsdot.shp` | 2912c921de 2011-05-02 Chris Forbes - add custom icon for harvester | download |
| `mods/ra/bits/gunfire2.shp` | 78b4f17baf 2013-08-04 reaperrr - Adds muzzle flash to all vehicles and ships with cannons. | download |
| `mods/ra/bits/harv.shp` | b4112dc2c3 2015-04-14 penev92 - Change RA harv.shp | download |
| `mods/ra/bits/harvempty.shp` | fe6a99c993 2011-07-17 Chris Forbes - fixed #42 -- show harvester fullness in sprites | download |
| `mods/ra/bits/harvhalf.shp` | fe6a99c993 2011-07-17 Chris Forbes - fixed #42 -- show harvester fullness in sprites | download |
| `mods/ra/bits/heli.shp` | bfca4897ea 2017-07-13 SoScared - Visual, Polish/Performance and Balance Tweak | download |
| `mods/ra/bits/hhusk.shp` | 59616671b6 2011-07-17 Chris Forbes - add harvester husks | download |
| `mods/ra/bits/hhusk2.shp` | 59616671b6 2011-07-17 Chris Forbes - add harvester husks | download |
| `mods/ra/bits/hind.shp` | e6668bbb59 2018-11-19 Inq8 - Improve Hind visibility | download |
| `mods/ra/bits/hospicon.shp` | 3aad269ac2 2014-01-31 Matthias Mailänder - added a hospital icon by Allen262 | download |
| `mods/ra/bits/iconchevrons.shp` | 78391e7c65 2015-08-15 Paul Chote - Replace icon veterancy overlay with new artwork. | download |
| `mods/ra/bits/invun.shp` | 43e99bf9f5 2012-09-27 Scott_NZ - Add heal animation for the heal crate | download |
| `mods/ra/bits/jmin.shp` | f2c630731e 2012-07-21 Matthias Mailänder - added some files from content.open-ra.org, fixes #2182 | download |
| `mods/ra/bits/levelup.shp` | 230625897a 2010-05-27 Chris Forbes - hack CrateEffect to have a custom offset; add levelup.shp; change > to >= in GainsExperience | download |
| `mods/ra/bits/lhus.shp` | 0cde0c1224 2015-04-11 Matthias Mailänder - add the light house | download |
| `mods/ra/bits/lrotorlg.shp` | d815348230 2013-08-05 reaperrr - Larger rotor for Hind. | download |
| `mods/ra/bits/mbAGUN.int` | fc2d0510d8 2014-03-19 Kanar - Adds minibibs for bibless buildings in RA and C&C. | download |
| `mods/ra/bits/mbAGUN.sno` | fc2d0510d8 2014-03-19 Kanar - Adds minibibs for bibless buildings in RA and C&C. | download |
| `mods/ra/bits/mbAGUN.tem` | fc2d0510d8 2014-03-19 Kanar - Adds minibibs for bibless buildings in RA and C&C. | download |
| `mods/ra/bits/mbFIX.int` | fc2d0510d8 2014-03-19 Kanar - Adds minibibs for bibless buildings in RA and C&C. | download |
| `mods/ra/bits/mbFIX.sno` | fc2d0510d8 2014-03-19 Kanar - Adds minibibs for bibless buildings in RA and C&C. | download |
| `mods/ra/bits/mbFIX.tem` | fc2d0510d8 2014-03-19 Kanar - Adds minibibs for bibless buildings in RA and C&C. | download |
| `mods/ra/bits/mbFTUR.int` | fc2d0510d8 2014-03-19 Kanar - Adds minibibs for bibless buildings in RA and C&C. | download |
| `mods/ra/bits/mbFTUR.sno` | fc2d0510d8 2014-03-19 Kanar - Adds minibibs for bibless buildings in RA and C&C. | download |
| `mods/ra/bits/mbFTUR.tem` | fc2d0510d8 2014-03-19 Kanar - Adds minibibs for bibless buildings in RA and C&C. | download |
| `mods/ra/bits/mbGAP.int` | fc2d0510d8 2014-03-19 Kanar - Adds minibibs for bibless buildings in RA and C&C. | download |
| `mods/ra/bits/mbGAP.sno` | fc2d0510d8 2014-03-19 Kanar - Adds minibibs for bibless buildings in RA and C&C. | download |
| `mods/ra/bits/mbGAP.tem` | fc2d0510d8 2014-03-19 Kanar - Adds minibibs for bibless buildings in RA and C&C. | download |
| `mods/ra/bits/mbGUN.int` | fc2d0510d8 2014-03-19 Kanar - Adds minibibs for bibless buildings in RA and C&C. | download |
| `mods/ra/bits/mbGUN.sno` | fc2d0510d8 2014-03-19 Kanar - Adds minibibs for bibless buildings in RA and C&C. | download |
| `mods/ra/bits/mbGUN.tem` | fc2d0510d8 2014-03-19 Kanar - Adds minibibs for bibless buildings in RA and C&C. | download |
| `mods/ra/bits/mbHOSP.int` | bf47543a0a 2014-06-06 Kanar - Adds/reworks some minibibs | download |
| `mods/ra/bits/mbHOSP.sno` | bf47543a0a 2014-06-06 Kanar - Adds/reworks some minibibs | download |
| `mods/ra/bits/mbHOSP.tem` | bf47543a0a 2014-06-06 Kanar - Adds/reworks some minibibs | download |
| `mods/ra/bits/mbIRON.int` | fc2d0510d8 2014-03-19 Kanar - Adds minibibs for bibless buildings in RA and C&C. | download |
| `mods/ra/bits/mbIRON.sno` | fc2d0510d8 2014-03-19 Kanar - Adds minibibs for bibless buildings in RA and C&C. | download |
| `mods/ra/bits/mbIRON.tem` | fc2d0510d8 2014-03-19 Kanar - Adds minibibs for bibless buildings in RA and C&C. | download |
| `mods/ra/bits/mbPBOX.int` | fc2d0510d8 2014-03-19 Kanar - Adds minibibs for bibless buildings in RA and C&C. | download |
| `mods/ra/bits/mbPBOX.sno` | fc2d0510d8 2014-03-19 Kanar - Adds minibibs for bibless buildings in RA and C&C. | download |
| `mods/ra/bits/mbPBOX.tem` | fc2d0510d8 2014-03-19 Kanar - Adds minibibs for bibless buildings in RA and C&C. | download |
| `mods/ra/bits/mbPDOX.int` | fc2d0510d8 2014-03-19 Kanar - Adds minibibs for bibless buildings in RA and C&C. | download |
| `mods/ra/bits/mbPDOX.sno` | fc2d0510d8 2014-03-19 Kanar - Adds minibibs for bibless buildings in RA and C&C. | download |
| `mods/ra/bits/mbPDOX.tem` | fc2d0510d8 2014-03-19 Kanar - Adds minibibs for bibless buildings in RA and C&C. | download |
| `mods/ra/bits/mbSAM.int` | fc2d0510d8 2014-03-19 Kanar - Adds minibibs for bibless buildings in RA and C&C. | download |
| `mods/ra/bits/mbSAM.sno` | fc2d0510d8 2014-03-19 Kanar - Adds minibibs for bibless buildings in RA and C&C. | download |
| `mods/ra/bits/mbSAM.tem` | fc2d0510d8 2014-03-19 Kanar - Adds minibibs for bibless buildings in RA and C&C. | download |
| `mods/ra/bits/mbSILO.int` | fc2d0510d8 2014-03-19 Kanar - Adds minibibs for bibless buildings in RA and C&C. | download |
| `mods/ra/bits/mbSILO.sno` | fc2d0510d8 2014-03-19 Kanar - Adds minibibs for bibless buildings in RA and C&C. | download |
| `mods/ra/bits/mbSILO.tem` | fc2d0510d8 2014-03-19 Kanar - Adds minibibs for bibless buildings in RA and C&C. | download |
| `mods/ra/bits/mbTSLA.int` | fc2d0510d8 2014-03-19 Kanar - Adds minibibs for bibless buildings in RA and C&C. | download |
| `mods/ra/bits/mbTSLA.sno` | fc2d0510d8 2014-03-19 Kanar - Adds minibibs for bibless buildings in RA and C&C. | download |
| `mods/ra/bits/mbTSLA.tem` | fc2d0510d8 2014-03-19 Kanar - Adds minibibs for bibless buildings in RA and C&C. | download |
| `mods/ra/bits/mcvhusk.shp` | ad12352f41 2011-07-27 Paul Chote - Add mcv husk by Daniel Hernandez | download |
| `mods/ra/bits/mgg.shp` | 2abb6421fe 2013-05-20 reaperrr - gap gen & mgg with shadows | download |
| `mods/ra/bits/mh60.shp` | 37325dbfc7 2019-05-29 Punsho - Reskin Hind with Black Hawk sprite | download |
| `mods/ra/bits/mh60icon.shp` | 37325dbfc7 2019-05-29 Punsho - Reskin Hind with Black Hawk sprite | download |
| `mods/ra/bits/mine.int` | e59d4568b0 2014-05-14 reaperrr - Adds ore mine graphics to interior tileset | download |
| `mods/ra/bits/missmake.shp` | 4ebaaf716d 2013-03-29 Matthias Mailänder - showcase everything we got icons for when dev cheats on | download |
| `mods/ra/bits/mpspawn.shp` | 5bd9273a7d 2011-02-15 Chris Forbes - fix issues with mpspawn and waypoint sprites | download |
| `mods/ra/bits/mslficon.shp` | 1986b31bcb 2015-01-23 ScottNZ - Add fake atek (atef), mslo (mslf) and pdox (pdof) structures to ra. Thanks to MicroBit for the icons. | download |
| `mods/ra/bits/msloicon2.shp` | da2d461aa8 2010-10-15 Chris Forbes - use fixed missile silo icon | download |
| `mods/ra/bits/napalm1.shp` | 7050fbc6e1 2012-04-30 Chris Forbes - add napalm1 SHP and wiring in ra/sequences | download |
| `mods/ra/bits/nopower.shp` | 19fadc3e0a 2010-01-27 Chris Forbes - fixed powerdown/blocked cursors | download |
| `mods/ra/bits/oilb.shp` | 6da5724af8 2011-06-25 Daniel Hernandez - added new 2x2 oil derrick | download |
| `mods/ra/bits/parach-shadow.shp` | 4a7687d7e4 2014-07-01 Matthias Mailänder - add a customizable parachute shadow | download |
| `mods/ra/bits/pdoficon.shp` | 1986b31bcb 2015-01-23 ScottNZ - Add fake atek (atef), mslo (mslf) and pdox (pdof) structures to ra. Thanks to MicroBit for the icons. | download |
| `mods/ra/bits/pip-disguise.shp` | 8a94caab3c 2015-06-13 Zimmermann Gyula - Implements a disguise decoration to Spies. Deprecates WithDecorationDisguised in favor of a refactor in Disguise. | download |
| `mods/ra/bits/pips2.shp` | 6c129eed3c 2011-07-24 Paul Chote - Split pip artwork into their own files and kill the hardcoded size/offset junk. Add a blue pip to support C&C blue tib. Fixes #836. | download |
| `mods/ra/bits/playersmoke.shp` | c811d24611 2010-04-14 Chris Forbes - recolor flares for owner | download |
| `mods/ra/bits/poweroff.shp` | 1d052bbfa2 2011-09-05 Curtis S - Powered down buildings now display a "powered off" animation | download |
| `mods/ra/bits/powrdead.shp` | 77d17ce761 2013-04-19 Matthias Mailänder - added building husks for RA | download |
| `mods/ra/bits/procdead.shp` | 77d17ce761 2013-04-19 Matthias Mailänder - added building husks for RA | download |
| `mods/ra/bits/proctop.shp` | edceb4e502 2014-05-13 reaperrr - Fixes harvester husk Z sorting on refinery. | download |
| `mods/ra/bits/railmine.sno` | 8cba84d47d 2014-09-25 s1ww - Reworked railmines for each tileset | download |
| `mods/ra/bits/railmine.tem` | 8cba84d47d 2014-09-25 s1ww - Reworked railmines for each tileset | download |
| `mods/ra/bits/rank.shp` | a259a0aca0 2010-05-27 Chris Forbes - hack in rank overlay; fix last rank; add an extra (elite) rank | download |
| `mods/ra/bits/rice.tem` | 71a4ebe8b7 2013-06-30 Matthias Mailänder - added a temperat rice field by @JOoGraphics | download |
| `mods/ra/bits/rushouse.shp` | e0a7819c54 2013-07-08 Matthias Mailänder - Added Russian city building by @JOoGraphics | download |
| `mods/ra/bits/sam2.shp` | 6db8952ce2 2014-03-19 reaperrr - Added some remap to bottom platform edge of sam site. | download |
| `mods/ra/bits/sbridge1.tem` | 9ce8c90d25 2012-07-28 Matthias Mailänder - added Harissons high-quality temperate terrain expansion | download |
| `mods/ra/bits/sbridge1d.tem` | 9ce8c90d25 2012-07-28 Matthias Mailänder - added Harissons high-quality temperate terrain expansion | download |
| `mods/ra/bits/sbridge1h.tem` | 9ce8c90d25 2012-07-28 Matthias Mailänder - added Harissons high-quality temperate terrain expansion | download |
| `mods/ra/bits/sbridge1x.tem` | 9ce8c90d25 2012-07-28 Matthias Mailänder - added Harissons high-quality temperate terrain expansion | download |
| `mods/ra/bits/sbridge2.tem` | 9ce8c90d25 2012-07-28 Matthias Mailänder - added Harissons high-quality temperate terrain expansion | download |
| `mods/ra/bits/sbridge2d.tem` | 9ce8c90d25 2012-07-28 Matthias Mailänder - added Harissons high-quality temperate terrain expansion | download |
| `mods/ra/bits/sbridge2h.tem` | 9ce8c90d25 2012-07-28 Matthias Mailänder - added Harissons high-quality temperate terrain expansion | download |
| `mods/ra/bits/sbridge2x.tem` | 9ce8c90d25 2012-07-28 Matthias Mailänder - added Harissons high-quality temperate terrain expansion | download |
| `mods/ra/bits/sbridge3.tem` | 9ce8c90d25 2012-07-28 Matthias Mailänder - added Harissons high-quality temperate terrain expansion | download |
| `mods/ra/bits/sbridge3d.tem` | 9ce8c90d25 2012-07-28 Matthias Mailänder - added Harissons high-quality temperate terrain expansion | download |
| `mods/ra/bits/sbridge3h.tem` | 9ce8c90d25 2012-07-28 Matthias Mailänder - added Harissons high-quality temperate terrain expansion | download |
| `mods/ra/bits/sbridge3x.tem` | 9ce8c90d25 2012-07-28 Matthias Mailänder - added Harissons high-quality temperate terrain expansion | download |
| `mods/ra/bits/sbridge4.tem` | 9ce8c90d25 2012-07-28 Matthias Mailänder - added Harissons high-quality temperate terrain expansion | download |
| `mods/ra/bits/sbridge4d.tem` | 9ce8c90d25 2012-07-28 Matthias Mailänder - added Harissons high-quality temperate terrain expansion | download |
| `mods/ra/bits/sbridge4h.tem` | 9ce8c90d25 2012-07-28 Matthias Mailänder - added Harissons high-quality temperate terrain expansion | download |
| `mods/ra/bits/sbridge4x.tem` | 9ce8c90d25 2012-07-28 Matthias Mailänder - added Harissons high-quality temperate terrain expansion | download |
| `mods/ra/bits/sc4.sno` | 107c37f33d 2012-12-03 Matthias Mailänder - RA: Snow craters fix by MrFlibble | download |
| `mods/ra/bits/scandy.aud` | ddef76e833 2022-04-29 Leo512bit - Added more thief voice lines | download |
| `mods/ra/bits/sccln.tem` | 9ce8c90d25 2012-07-28 Matthias Mailänder - added Harissons high-quality temperate terrain expansion | download |
| `mods/ra/bits/sccls.tem` | 9ce8c90d25 2012-07-28 Matthias Mailänder - added Harissons high-quality temperate terrain expansion | download |
| `mods/ra/bits/sccnl.tem` | 9ce8c90d25 2012-07-28 Matthias Mailänder - added Harissons high-quality temperate terrain expansion | download |
| `mods/ra/bits/sccnr.tem` | 9ce8c90d25 2012-07-28 Matthias Mailänder - added Harissons high-quality temperate terrain expansion | download |
| `mods/ra/bits/sccrn.tem` | 9ce8c90d25 2012-07-28 Matthias Mailänder - added Harissons high-quality temperate terrain expansion | download |
| `mods/ra/bits/sccrs.tem` | 9ce8c90d25 2012-07-28 Matthias Mailänder - added Harissons high-quality temperate terrain expansion | download |
| `mods/ra/bits/sccsl.tem` | 9ce8c90d25 2012-07-28 Matthias Mailänder - added Harissons high-quality temperate terrain expansion | download |
| `mods/ra/bits/sccsr.tem` | 9ce8c90d25 2012-07-28 Matthias Mailänder - added Harissons high-quality temperate terrain expansion | download |
| `mods/ra/bits/sclean1.aud` | ddef76e833 2022-04-29 Leo512bit - Added more thief voice lines | download |
| `mods/ra/bits/scrate.shp` | 3cc3a7dd65 2013-05-20 reaperrr - crates with shadow and wood crate palette fix | download |
| `mods/ra/bits/sh57.tem` | 9ce8c90d25 2012-07-28 Matthias Mailänder - added Harissons high-quality temperate terrain expansion | download |
| `mods/ra/bits/sh58.tem` | 9ce8c90d25 2012-07-28 Matthias Mailänder - added Harissons high-quality temperate terrain expansion | download |
| `mods/ra/bits/sh59.tem` | 9ce8c90d25 2012-07-28 Matthias Mailänder - added Harissons high-quality temperate terrain expansion | download |
| `mods/ra/bits/sh60.tem` | 9ce8c90d25 2012-07-28 Matthias Mailänder - added Harissons high-quality temperate terrain expansion | download |
| `mods/ra/bits/sh61.tem` | 9ce8c90d25 2012-07-28 Matthias Mailänder - added Harissons high-quality temperate terrain expansion | download |
| `mods/ra/bits/sh62.tem` | 9ce8c90d25 2012-07-28 Matthias Mailänder - added Harissons high-quality temperate terrain expansion | download |
| `mods/ra/bits/sh63.tem` | 9ce8c90d25 2012-07-28 Matthias Mailänder - added Harissons high-quality temperate terrain expansion | download |
| `mods/ra/bits/sh64.tem` | 9ce8c90d25 2012-07-28 Matthias Mailänder - added Harissons high-quality temperate terrain expansion | download |
| `mods/ra/bits/silo2.shp` | 713260a49f 2013-04-19 Matthias Mailänder - added more silo fillstep animations | download |
| `mods/ra/bits/silomake.shp` | fbe06df9e6 2013-05-20 reaperrr - silo gfx improvements | download |
| `mods/ra/bits/silppk.aud` | 7d4186c236 2012-04-06 Chris Forbes - add SilencedPPK weapon for spy | download |
| `mods/ra/bits/snowhut.shp` | 8e9364e5a4 2013-12-21 MicroGraphics - snowhut | download |
| `mods/ra/bits/ssteal1.aud` | ddef76e833 2022-04-29 Leo512bit - Added more thief voice lines | download |
| `mods/ra/bits/stnkicon.shp` | 687f1f9bcd 2014-03-10 ScottNZ - Implement phase transport | download |
| `mods/ra/bits/tag-spy.shp` | 4961b0943b 2020-03-18 Matthias Mailänder - Add a text base spy disguise decoration. | download |
| `mods/ra/bits/tanktrap1.shp` | 6a7f8c9416 2013-06-30 Matthias Mailänder - added a tank trap by @CombineCR | download |
| `mods/ra/bits/tanktrap2.shp` | 8fedf1f0f4 2013-06-30 Matthias Mailänder - added Czech hedgehog by @JOoGraphics | download |
| `mods/ra/bits/tenficon.shp` | 9ac3d7507c 2019-06-30 Punsho - Add Fake Allied Barracks to RA | download |
| `mods/ra/bits/tent.sno` | 9ce25bd400 2014-03-02 Matthias Mailänder - added a snow variation of TENT by Kilkakon | download |
| `mods/ra/bits/tent.tem` | 557b479f40 2015-03-28 reaperrr - Makes ra tent art tileset-specific again | download |
| `mods/ra/bits/tentmake.sno` | 9ce25bd400 2014-03-02 Matthias Mailänder - added a snow variation of TENT by Kilkakon | download |
| `mods/ra/bits/tentmake.tem` | 557b479f40 2015-03-28 reaperrr - Makes ra tent art tileset-specific again | download |
| `mods/ra/bits/tire.shp` | b5327a2cb7 2010-05-30 Chris Forbes - add tire particle sprite (4 frames) | download |
| `mods/ra/bits/tran1husk.shp` | 55651738a7 2012-07-30 Chris Forbes - add heli crash shps | download |
| `mods/ra/bits/tran2.shp` | b8ed9d7da5 2010-09-09 Chris Forbes - (katzsmile) Added: Soviet Flak Truck with new SHP and Cameo | download |
| `mods/ra/bits/tran2husk.shp` | 55651738a7 2012-07-30 Chris Forbes - add heli crash shps | download |
| `mods/ra/bits/truk.shp` | 88949024e6 2012-12-03 Matthias Mailänder - RA: add shadow to supply truck | download |
| `mods/ra/bits/unitsto.aud` | 5eb91d2811 2014-04-05 ScottNZ - Add hijacker | download |
| `mods/ra/bits/utilpol1.shp` | e7708e7b4a 2013-06-30 Matthias Mailänder - Added Utility Poles by @CombineCR | download |
| `mods/ra/bits/utilpol2.shp` | e7708e7b4a 2013-06-30 Matthias Mailänder - Added Utility Poles by @CombineCR | download |
| `mods/ra/bits/waypoint.shp` | 5bd9273a7d 2011-02-15 Chris Forbes - fix issues with mpspawn and waypoint sprites | download |
| `mods/ra/bits/wcrate.shp` | 3cc3a7dd65 2013-05-20 reaperrr - crates with shadow and wood crate palette fix | download |
| `mods/ra/bits/weap3.shp` | 2c66e5352e 2013-04-19 Matthias Mailänder - added more frames to the RA weapon factory doors | download |
| `mods/ra/bits/windmill.shp` | cae7d7cffa 2015-04-11 Matthias Mailänder - add the windmill | download |
| `mods/ra/bits/wpiff.shp` | e80785b59b 2013-08-08 reaperrr - Adds water piff(s) animations to machine gun/pistol/rifle weapons. | download |
| `mods/ra/bits/wpifpif.shp` | e80785b59b 2013-08-08 reaperrr - Adds water piff(s) animations to machine gun/pistol/rifle weapons. | download |
| `mods/ra/bits/xcratea.shp` | e9e2e353f7 2013-12-15 Paul Chote - Change crate graphics for christmas. Closes #4296. | download |
| `mods/ra/bits/xcrateb.shp` | e9e2e353f7 2013-12-15 Paul Chote - Change crate graphics for christmas. Closes #4296. | download |
| `mods/ra/bits/xcratec.shp` | e9e2e353f7 2013-12-15 Paul Chote - Change crate graphics for christmas. Closes #4296. | download |
| `mods/ra/bits/xcrated.shp` | e9e2e353f7 2013-12-15 Paul Chote - Change crate graphics for christmas. Closes #4296. | download |
| `mods/ra/bits/yak.shp` | 579bafd439 2013-05-20 reaperrr - Yak shp fix | download |
| `mods/ra/bits/yrotorlg.shp` | 37325dbfc7 2019-05-29 Punsho - Reskin Hind with Black Hawk sprite | download |
| `mods/ra/bits/zombicon.shp` | 0cf63d9899 2013-06-24 Matthias Mailänder - added a Zombie unit | download |
| `mods/ra/bits/zombie.shp` | 0cf63d9899 2013-06-24 Matthias Mailänder - added a Zombie unit | download |
| `mods/ra/icon-2x.png` | fb7c781a66 2020-02-15 Paul Chote - Add multi-resolution mod icon support. | download |
| `mods/ra/icon-3x.png` | fb7c781a66 2020-02-15 Paul Chote - Add multi-resolution mod icon support. | download |
| `mods/ra/icon.png` | 4d982f00e4 2017-01-20 Paul Chote - Add 32px icons for default mods. | download |
| `mods/ra/maps/bomber-john/brick.shp` | 0167bbfbaa 2013-03-14 Paul Chote - Move weapon/turret definitions out of AttackBase. | download |
| `mods/ra/maps/bomber-john/miner.shp` | 0167bbfbaa 2013-03-14 Paul Chote - Move weapon/turret definitions out of AttackBase. | download |
| `mods/ra/maps/chernobyl/plains.pal` | 0320dcdef9 2020-09-14 abcdefg30 - Revise the RA map pool | download |
| `mods/ra/maps/chernobyl/temperat.pal` | 0320dcdef9 2020-09-14 abcdefg30 - Revise the RA map pool | download |
| `mods/ra/maps/fort-lonestar/rain.aud` | f6ae27b4b7 2015-07-05 Matthias Mailänder - add thunderstorm effects to fort lonestar | ship |
| `mods/ra/maps/fort-lonestar/sniper.shp` | 2c938bc305 2012-04-30 Chris Forbes - add sniper SHP | download |
| `mods/ra/maps/fort-lonestar/snipericon.shp` | 02f28f54c7 2012-07-27 Matthias Mailänder - add sniper to ra | download |
| `mods/ra/maps/fort-lonestar/thunder-ambient.aud` | befdac8237 2015-07-30 Zimmermann Gyula - Replace ambient sounds from unknown sources with CC0 licensed ones | ship |
| `mods/ra/maps/fort-lonestar/thunder1.aud` | befdac8237 2015-07-30 Zimmermann Gyula - Replace ambient sounds from unknown sources with CC0 licensed ones | ship |
| `mods/ra/maps/fort-lonestar/thunder2.aud` | befdac8237 2015-07-30 Zimmermann Gyula - Replace ambient sounds from unknown sources with CC0 licensed ones | ship |
| `mods/ra/maps/fort-lonestar/thunder3.aud` | befdac8237 2015-07-30 Zimmermann Gyula - Replace ambient sounds from unknown sources with CC0 licensed ones | ship |
| `mods/ra/maps/fort-lonestar/thunder4.aud` | befdac8237 2015-07-30 Zimmermann Gyula - Replace ambient sounds from unknown sources with CC0 licensed ones | ship |
| `mods/ra/maps/fort-lonestar/thunder5.aud` | befdac8237 2015-07-30 Zimmermann Gyula - Replace ambient sounds from unknown sources with CC0 licensed ones | ship |
| `mods/ra/uibits/dialog.png` | 4c5f02b801 2010-02-05 Paul Chote - Unfail dialog drawing | download |
| `mods/ra/uibits/glyphs-2x.png` | 1282650274 2020-01-12 Paul Chote - Rework RA Chrome in preparation for UI scaling. | download |
| `mods/ra/uibits/glyphs-3x.png` | 1282650274 2020-01-12 Paul Chote - Rework RA Chrome in preparation for UI scaling. | download |
| `mods/ra/uibits/glyphs.png` | 1282650274 2020-01-12 Paul Chote - Rework RA Chrome in preparation for UI scaling. | download |
| `mods/ra/uibits/loadscreen-2x.png` | 1282650274 2020-01-12 Paul Chote - Rework RA Chrome in preparation for UI scaling. | download |
| `mods/ra/uibits/loadscreen-3x.png` | 1282650274 2020-01-12 Paul Chote - Rework RA Chrome in preparation for UI scaling. | download |
| `mods/ra/uibits/loadscreen.png` | dbd076543b 2010-08-25 Paul Chote - Low level loadscreen displays earlier | download |
| `mods/ra/uibits/sidebar.png` | 1282650274 2020-01-12 Paul Chote - Rework RA Chrome in preparation for UI scaling. | download |
