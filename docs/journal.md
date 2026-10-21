# Journal des échanges et des décisions

Ce qui a été demandé, choisi ou décidé pendant le développement, du plus ancien au plus récent.
« Provisoire » : choix fait sans l'utilisateur, à confirmer.

## 21/10/2026 — démarrage du premier jet

**Demande** : construire une première version utile de bout en bout, par étapes (un commit par
étape, tests de la logique), sans rien pousser ni toucher aux autres dépôts.

**Pilotage** : l'utilisateur a confié le suivi à une autre session (« pilote »), qui répond aux
questions de choix jusqu'au premier jet ; il reprendra la main ensuite.

**Premier jet attendu** (pilote) : l'exécutable se lance ; on y définit un gardien (déclencheur
heure ou fichier, action script) ; il s'exécute et son résultat arrive dans la boîte de résultats,
visible dans l'interface ; l'historique survit à un redémarrage ; `cargo test` au vert ; le README
explique comment le lancer. Les sessions Claude non interactives viennent après les scripts, testées
avec un faux exécutable.

**Choix (provisoires, validés par le pilote)** :
- **Hors application** : les tâches ne tournent que si Patronus est ouvert. Les exécutions manquées
  sont signalées à l'ouverture (une entrée « manquée » dans la boîte), ou rattrapées si la tâche le
  demande. Service ou tâche planifiée Windows : plus tard.
- **Déclencheurs du premier jet** : heure (et intervalle), fichier modifié, à la main. Branche
  distante ensuite si le temps le permet.
- **Structure** : workspace de crates, `patronus-core` (logique sans interface, testable) et
  `patronus-app` (bibliothèque + exécutable `patronus`), comme DragonTree.
- **Coût** : tokens et montant « équivalent API » rapporté par Claude ; le budget d'une tâche
  s'applique à cet équivalent.

**Choix d'implémentation (miens)** :
- Format des tâches : celui de `docs/spec.md` §8, champs en français ; un déclencheur se reconnaît à
  sa clé (`heure`, `toutes_les_minutes`, `fichiers`) ; les champs inconnus sont gardés.
- Une tâche supprimée est mise de côté (`<id>.json.supprimee`), pas effacée.
- Une commande ou une consigne qui ressemble à un secret (jeton connu, `password=`…) rend la tâche
  invalide.
- Compilation avec la toolchain gnu : même préparation que Fawkes (`tools\setup.ps1`, recopié).

## 21/10/2026 — premier jet

**Pilote** : faire l'interface minimale tôt, juste après l'exécution des scripts ; vérifier
l'interface par une capture faite par l'application elle-même (l'écran du poste est en veille).
Fait : `PATRONUS_CAPTURE`, avec un dossier de données isolé (`PATRONUS_DATA_DIR`).

**Choix d'implémentation (miens, provisoires)** :
- Un fichier modifié par la tâche pendant qu'elle tourne ne la redéclenche pas (sinon une tâche qui
  écrit dans le dossier qu'elle surveille bouclerait) ; c'est noté dans son journal.
- Les départs manqués d'un même déclencheur sont regroupés en une seule entrée « manquée » (ou un
  seul rattrapage).
- Une exécution laissée « en cours » par un arrêt de Patronus devient « interrompue » au démarrage.
- Rétention : les résultats lus ou archivés de plus de 30 jours sont supprimés (réglable) ; les non
  lus restent.
- Notification Windows : bulle d'une icône de zone de notification, ajoutée à la première
  notification.
- `patronus lancer <tâche>` exécute la tâche dans le processus appelant, sans passer par la fenêtre
  ouverte : en attendant le canal nommé, deux exécutions de la même tâche restent possibles si on
  lance en ligne de commande pendant qu'elle tourne dans la fenêtre.
- Contrat local : proposition dans `docs/contrat-local.md`.
