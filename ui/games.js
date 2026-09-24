// PacketYellow game catalog (200+ online games).
// Most game servers only speak UDP, which a no-admin TCP ping can't reach. So every game is mapped to
// the server *regions* it's hosted in, and we ping a TCP endpoint in that same datacenter/city.
// Latency to a city is what decides your in-game ping, so this is an accurate proxy.
// Where a game has a real TCP server (Supercell 9339, Battle.net 1119, Riot 5223, Minecraft 25565,
// Steam connection servers) we ping that directly. Duplicate hosts are pinged once and shared.
(() => {
  const A = r => `dynamodb.${r}.amazonaws.com`;
  const R = {
    bh: ['خاورمیانه · بحرین', A('me-south-1'), 443],
    uae: ['خاورمیانه · امارات', A('me-central-1'), 443],
    tlv: ['خاورمیانه · تل‌آویو', A('il-central-1'), 443],
    fra: ['اروپا · فرانکفورت', A('eu-central-1'), 443],
    irl: ['اروپا · ایرلند', A('eu-west-1'), 443],
    lon: ['اروپا · لندن', A('eu-west-2'), 443],
    par: ['اروپا · پاریس', A('eu-west-3'), 443],
    sto: ['اروپا · استکهلم', A('eu-north-1'), 443],
    mil: ['اروپا · میلان', A('eu-south-1'), 443],
    mum: ['آسیا · بمبئی', A('ap-south-1'), 443],
    sg: ['آسیا · سنگاپور', A('ap-southeast-1'), 443],
  };
  delete R.tlv; // not useful from Iran, kept out on purpose
  const P = {
    me_eu: [R.bh, R.uae, R.fra, R.lon],
    eu: [R.fra, R.irl, R.lon, R.sto],
    eu_me: [R.fra, R.par, R.mil, R.bh],
    asia_me: [R.bh, R.uae, R.mum, R.sg, R.fra],
    riot: [['Riot · EUW (آمستردام)', 'euw1.chat.si.riotgames.com', 5223], ['Riot · EUNE', 'eun1.chat.si.riotgames.com', 5223], ['Riot · ترکیه', 'tr1.chat.si.riotgames.com', 5223], R.bh],
    valorant: [R.bh, ['Riot · ترکیه', 'tr1.chat.si.riotgames.com', 5223], R.fra, R.sto, R.par],
    bnet: [['Battle.net · اروپا', 'eu.actual.battle.net', 1119], R.fra, R.par],
    supercell: [['Supercell · سرور بازی', 'gamea.clashofclans.com', 9339], R.fra, R.bh],
    clashroyale: [['Supercell · Clash Royale', 'game.clashroyaleapp.com', 9339], R.fra],
    brawlstars: [['Supercell · Brawl Stars', 'game.brawlstarsgame.com', 9339], R.fra],
    hoyo: [['HoYoverse · اروپا', 'oseurodispatch.yuanshen.com', 443], ['HoYoverse · آسیا', 'osasiadispatch.yuanshen.com', 443]],
    hypixel: [['Hypixel', 'mc.hypixel.net', 25565], ['CubeCraft', 'play.cubecraft.net', 25565], R.fra],
    rockstar: [['Rockstar Online Services', 'prod.ros.rockstargames.com', 443], R.fra, R.bh],
    ubi: [['Ubisoft Connect', 'public-ubiservices.ubi.com', 443], R.fra, R.par, R.bh],
    ea: [R.fra, R.irl, R.lon, R.bh],
    epic: [R.bh, R.fra, R.lon, R.par],
    bungie: [['Bungie', 'www.bungie.net', 443], R.fra, R.irl],
    roblox: [['Roblox', 'apis.roblox.com', 443], R.fra, R.lon],
    chess: [['Chess.com', 'www.chess.com', 443], ['Lichess', 'lichess.org', 443]],
    steam: 'steam',
  };
  const C = { fps: 'شوتر', br: 'بتل رویال', moba: 'MOBA', sport: 'ورزشی و ریسینگ', mmo: 'MMO و نقش‌آفرینی', surv: 'بقا و سندباکس', strat: 'استراتژی و کارتی', fight: 'مبارزه‌ای', mobile: 'موبایل', party: 'پارتی و دیگر' };
  const L = [
    // --- shooters
    ['Valorant', 'fps', 'valorant'], ['Counter-Strike 2', 'fps', 'steam'], ['Call of Duty: Warzone', 'fps', 'me_eu'], ['Call of Duty: Black Ops 6', 'fps', 'me_eu'],
    ['Call of Duty: Modern Warfare III', 'fps', 'me_eu'], ['Overwatch 2', 'fps', 'bnet'], ['Rainbow Six Siege', 'fps', 'ubi'], ['Apex Legends', 'fps', 'ea'],
    ['Battlefield 2042', 'fps', 'ea'], ['Battlefield V', 'fps', 'ea'], ['Battlefield 1', 'fps', 'ea'], ['Destiny 2', 'fps', 'bungie'], ['Halo Infinite', 'fps', 'eu_me'],
    ['Team Fortress 2', 'fps', 'steam'], ['Left 4 Dead 2', 'fps', 'steam'], ['Deadlock', 'fps', 'steam'], ['Escape from Tarkov', 'fps', 'eu'], ['Hunt: Showdown 1896', 'fps', 'eu'],
    ['Insurgency: Sandstorm', 'fps', 'eu'], ['Squad', 'fps', 'eu'], ['Hell Let Loose', 'fps', 'eu'], ['The Finals', 'fps', 'me_eu'], ['XDefiant', 'fps', 'ubi'],
    ['Splitgate 2', 'fps', 'eu'], ['Paladins', 'fps', 'eu'], ['Warframe', 'fps', 'eu'], ['Payday 3', 'fps', 'eu'], ['Ready or Not', 'fps', 'eu'], ['Delta Force', 'fps', 'asia_me'],
    ['Marvel Rivals', 'fps', 'asia_me'], ['Helldivers 2', 'fps', 'eu'], ['CrossFire', 'fps', 'asia_me'], ['Point Blank', 'fps', 'asia_me'], ['Arena Breakout: Infinite', 'fps', 'asia_me'],
    ['Rogue Company', 'fps', 'epic'], ['Spectre Divide', 'fps', 'eu'], ['Chivalry 2', 'fps', 'eu'], ['Sea of Thieves', 'fps', 'eu_me'], ['Gears 5', 'fps', 'eu_me'],
    ['Enlisted', 'fps', 'eu'], ['Battlebit Remastered', 'fps', 'eu'], ['Black Squad', 'fps', 'asia_me'], ['Counter-Strike: Source', 'fps', 'steam'], ['Day of Defeat: Source', 'fps', 'steam'],
    ['Titanfall 2', 'fps', 'ea'], ['Star Wars Battlefront II', 'fps', 'ea'], ['Ghost Recon Breakpoint', 'fps', 'ubi'], ['The Division 2', 'fps', 'ubi'], ['Far Cry 6 (co-op)', 'fps', 'ubi'],
    // --- battle royale
    ['Fortnite', 'br', 'epic'], ['PUBG: Battlegrounds', 'br', 'me_eu'], ['Naraka: Bladepoint', 'br', 'asia_me'], ['Super People', 'br', 'asia_me'], ['Realm Royale', 'br', 'eu'],
    ['Fall Guys', 'br', 'epic'], ['Farlight 84', 'br', 'asia_me'], ['Blood Strike', 'br', 'asia_me'], ['Warzone Mobile', 'br', 'me_eu'], ['Spellbreak', 'br', 'epic'],
    // --- MOBA
    ['League of Legends', 'moba', 'riot'], ['Dota 2', 'moba', 'steam'], ['Teamfight Tactics', 'moba', 'riot'], ['Heroes of the Storm', 'moba', 'bnet'], ['Smite 2', 'moba', 'eu'],
    ['Predecessor', 'moba', 'eu'], ['Pokémon Unite', 'moba', 'asia_me'], ['Honor of Kings', 'moba', 'asia_me'], ['Onmyoji Arena', 'moba', 'asia_me'], ['Dota Underlords', 'moba', 'steam'],
    ['Battlerite', 'moba', 'eu'], ['Heroes of Newerth', 'moba', 'eu'],
    // --- sports & racing
    ['EA Sports FC 25', 'sport', 'ea'], ['EA Sports FC Online', 'sport', 'asia_me'], ['eFootball', 'sport', 'me_eu'], ['Rocket League', 'sport', 'epic'], ['NBA 2K25', 'sport', 'eu_me'],
    ['Madden NFL 25', 'sport', 'ea'], ['NHL 25', 'sport', 'ea'], ['F1 24', 'sport', 'ea'], ['EA Sports WRC', 'sport', 'ea'], ['Forza Horizon 5', 'sport', 'eu_me'],
    ['Forza Motorsport', 'sport', 'eu_me'], ['Gran Turismo 7', 'sport', 'eu'], ['Need for Speed Unbound', 'sport', 'ea'], ['Need for Speed Heat', 'sport', 'ea'], ['The Crew Motorfest', 'sport', 'ubi'],
    ['Trackmania', 'sport', 'ubi'], ['iRacing', 'sport', 'eu'], ['Assetto Corsa Competizione', 'sport', 'steam'], ['WWE 2K24', 'sport', 'eu_me'], ['UFC 5', 'sport', 'ea'],
    ['TopSpin 2K25', 'sport', 'eu_me'], ['PGA Tour 2K23', 'sport', 'eu_me'], ['Riders Republic', 'sport', 'ubi'], ['Steep', 'sport', 'ubi'], ['Wreckfest', 'sport', 'steam'],
    // --- MMO / RPG
    ['World of Warcraft', 'mmo', 'bnet'], ['Diablo IV', 'mmo', 'bnet'], ['Diablo II: Resurrected', 'mmo', 'bnet'], ['Diablo Immortal', 'mmo', 'bnet'], ['Final Fantasy XIV', 'mmo', 'eu'],
    ['Guild Wars 2', 'mmo', 'eu'], ['The Elder Scrolls Online', 'mmo', 'eu'], ['Black Desert', 'mmo', 'eu'], ['Lost Ark', 'mmo', 'eu'], ['New World: Aeternum', 'mmo', 'eu'],
    ['Throne and Liberty', 'mmo', 'eu'], ['Albion Online', 'mmo', 'eu'], ['RuneScape', 'mmo', 'eu'], ['Old School RuneScape', 'mmo', 'eu'], ['EVE Online', 'mmo', 'eu'],
    ['Star Wars: The Old Republic', 'mmo', 'eu'], ['The Lord of the Rings Online', 'mmo', 'eu'], ['Path of Exile', 'mmo', 'eu'], ['Path of Exile 2', 'mmo', 'eu'], ['Genshin Impact', 'mmo', 'hoyo'],
    ['Honkai: Star Rail', 'mmo', 'hoyo'], ['Zenless Zone Zero', 'mmo', 'hoyo'], ['Honkai Impact 3rd', 'mmo', 'hoyo'], ['Wuthering Waves', 'mmo', 'asia_me'], ['Tower of Fantasy', 'mmo', 'asia_me'],
    ['MapleStory', 'mmo', 'eu'], ['Metin2', 'mmo', 'eu'], ['Silkroad Online', 'mmo', 'eu'], ['Blade & Soul NEO', 'mmo', 'eu'], ['Tarisland', 'mmo', 'asia_me'],
    ['Lineage 2', 'mmo', 'eu'], ['Aion', 'mmo', 'eu'], ['Tibia', 'mmo', 'eu'], ['Dofus', 'mmo', 'eu'], ['Monster Hunter Wilds', 'mmo', 'eu_me'],
    ['Phantasy Star Online 2 NGS', 'mmo', 'eu'], ['Once Human', 'mmo', 'asia_me'], ['Dune: Awakening', 'mmo', 'eu'], ['Elden Ring (online)', 'mmo', 'eu_me'], ['Dark Souls III (online)', 'mmo', 'eu_me'],
    ['Warhammer Online', 'mmo', 'eu'], ['Neverwinter', 'mmo', 'eu'], ['Star Trek Online', 'mmo', 'eu'], ['Mabinogi', 'mmo', 'eu'], ['Ragnarok Online', 'mmo', 'asia_me'],
    // --- survival / sandbox
    ['Minecraft (Java)', 'surv', 'hypixel'], ['Minecraft Bedrock', 'surv', 'hypixel'], ['Roblox', 'surv', 'roblox'], ['Rust', 'surv', 'eu'], ['DayZ', 'surv', 'eu'],
    ['Arma 3', 'surv', 'eu'], ['Arma Reforger', 'surv', 'eu'], ['Terraria', 'surv', 'eu'], ['Valheim', 'surv', 'eu'], ['ARK: Survival Ascended', 'surv', 'eu_me'],
    ['7 Days to Die', 'surv', 'eu'], ["Don't Starve Together", 'surv', 'eu'], ['Sons of the Forest', 'surv', 'eu'], ['Grounded', 'surv', 'eu_me'], ['Enshrouded', 'surv', 'eu'],
    ['Conan Exiles', 'surv', 'eu'], ["No Man's Sky", 'surv', 'eu'], ['Satisfactory', 'surv', 'eu'], ['Palworld', 'surv', 'eu_me'], ['Project Zomboid', 'surv', 'eu'],
    ['V Rising', 'surv', 'eu'], ["Garry's Mod", 'surv', 'steam'], ['Lethal Company', 'surv', 'eu'], ['Phasmophobia', 'surv', 'eu'], ['Dead by Daylight', 'surv', 'me_eu'],
    ['Among Us', 'surv', 'eu_me'], ['Last Day on Earth', 'surv', 'eu'], ['Unturned', 'surv', 'steam'], ['SCUM', 'surv', 'eu'], ['The Isle', 'surv', 'eu'],
    // --- strategy & cards
    ['Hearthstone', 'strat', 'bnet'], ['StarCraft II', 'strat', 'bnet'], ['Warcraft Rumble', 'strat', 'bnet'], ['Clash of Clans', 'strat', 'supercell'], ['Clash Royale', 'strat', 'clashroyale'],
    ['Boom Beach', 'strat', 'supercell'], ['Hay Day', 'strat', 'supercell'], ['Squad Busters', 'strat', 'supercell'], ['Age of Empires IV', 'strat', 'eu_me'], ['Age of Empires II: DE', 'strat', 'eu_me'],
    ['Company of Heroes 3', 'strat', 'eu'], ['Total War: Warhammer III', 'strat', 'eu'], ['Civilization VI', 'strat', 'eu'], ['Rise of Kingdoms', 'strat', 'asia_me'], ['Lords Mobile', 'strat', 'asia_me'],
    ['Call of Dragons', 'strat', 'asia_me'], ['State of Survival', 'strat', 'asia_me'], ['Whiteout Survival', 'strat', 'asia_me'], ['Evony', 'strat', 'asia_me'], ['Marvel Snap', 'strat', 'eu'],
    ['Legends of Runeterra', 'strat', 'riot'], ['Gwent', 'strat', 'eu'], ['Magic: The Gathering Arena', 'strat', 'eu'], ['Yu-Gi-Oh! Master Duel', 'strat', 'asia_me'], ['Pokémon TCG Live', 'strat', 'eu'],
    ['Chess.com', 'strat', 'chess'], ['Lichess', 'strat', 'chess'], ['Travian', 'strat', 'eu'], ['Forge of Empires', 'strat', 'eu'], ['Top War', 'strat', 'asia_me'],
    ['World of Tanks', 'strat', 'eu'], ['World of Warships', 'strat', 'eu'], ['World of Tanks Blitz', 'strat', 'eu'], ['War Thunder', 'strat', 'eu'], ['Crossout', 'strat', 'eu'],
    ['War Robots', 'strat', 'eu'], ['Mech Arena', 'strat', 'eu'], ['Stormgate', 'strat', 'eu'], ['Northgard', 'strat', 'eu'], ['Vikings: War of Clans', 'strat', 'eu'],
    // --- fighting
    ['Tekken 8', 'fight', 'eu_me'], ['Street Fighter 6', 'fight', 'eu_me'], ['Mortal Kombat 1', 'fight', 'eu_me'], ['Brawlhalla', 'fight', 'ubi'], ['MultiVersus', 'fight', 'eu_me'],
    ['Dragon Ball FighterZ', 'fight', 'eu_me'], ['Guilty Gear Strive', 'fight', 'eu_me'], ['Shadow Fight 4: Arena', 'fight', 'eu'], ['For Honor', 'fight', 'ubi'], ['Mordhau', 'fight', 'eu'],
    // --- mobile
    ['PUBG Mobile', 'mobile', 'asia_me'], ['Call of Duty: Mobile', 'mobile', 'asia_me'], ['Free Fire', 'mobile', 'asia_me'], ['Free Fire MAX', 'mobile', 'asia_me'], ['Mobile Legends: Bang Bang', 'mobile', 'asia_me'],
    ['League of Legends: Wild Rift', 'mobile', 'asia_me'], ['Arena of Valor', 'mobile', 'asia_me'], ['Brawl Stars', 'mobile', 'brawlstars'], ['EA Sports FC Mobile', 'mobile', 'ea'], ['Standoff 2', 'mobile', 'eu'],
    ['Critical Ops', 'mobile', 'eu'], ['Arena Breakout', 'mobile', 'asia_me'], ['Stumble Guys', 'mobile', 'eu'], ['Asphalt Legends Unite', 'mobile', 'eu'], ['Real Racing 3', 'mobile', 'ea'],
    ['8 Ball Pool', 'mobile', 'eu'], ['Carrom Pool', 'mobile', 'asia_me'], ['Ludo King', 'mobile', 'asia_me'], ['Pokémon GO', 'mobile', 'eu'], ['Sausage Man', 'mobile', 'asia_me'],
    ['Eggy Party', 'mobile', 'asia_me'], ['Knives Out', 'mobile', 'asia_me'], ['Pixel Gun 3D', 'mobile', 'eu'], ['Zooba', 'mobile', 'eu'], ['Mini Militia', 'mobile', 'asia_me'],
    ['Dream League Soccer', 'mobile', 'eu'], ['Top Eleven', 'mobile', 'eu'], ['Golf Clash', 'mobile', 'eu'], ['Shadowgun Legends', 'mobile', 'eu'], ['Modern Combat 5', 'mobile', 'eu'],
    // --- party & other
    ['GTA Online', 'party', 'rockstar'], ['Red Dead Online', 'party', 'rockstar'], ['UNO!', 'party', 'ubi'], ['Gartic Phone', 'party', 'eu'], ['Jackbox Party Pack', 'party', 'eu'],
    ['Overcooked! 2', 'party', 'eu'], ['It Takes Two', 'party', 'ea'], ['Rec Room', 'party', 'eu'], ['VRChat', 'party', 'eu'], ['Minecraft Dungeons', 'party', 'eu_me'],
    ['Pummel Party', 'party', 'eu'], ['Human: Fall Flat', 'party', 'eu'], ['Stardew Valley (co-op)', 'party', 'eu'], ['Deep Rock Galactic', 'party', 'eu'], ['Vermintide 2', 'party', 'eu'],
  ];
  const seen = new Set();
  const GAMES = [];
  for (const [name, cat, prof] of L) {
    if (seen.has(name)) continue; seen.add(name);
    const p = P[prof];
    GAMES.push(p === 'steam' ? { name, cat, steam: true, servers: [] } : { name, cat, servers: p });
  }
  window.PY_GAMES = { GAMES, CATS: C };
})();
