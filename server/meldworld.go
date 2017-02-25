package main

import (
	"encoding/json"
	"html/template"
	"io/ioutil"
	"net/http"

	"github.com/blanklabel/meldworld/model"
	"github.com/gorilla/websocket"
	log "github.com/sirupsen/logrus"
)

var upgrader = websocket.Upgrader{} // use default options
var homeTemplate = template.Must(template.ParseFiles("home.html"))

var gh = GameHub{
	clients:      make(map[string]*model.Player),
	broadcast:    make(chan *model.ClientMessage),
	register:     make(chan *model.Player),
	unregister:   make(chan model.Player),
	entityaction: make(chan *model.EntityAction),
	actionqueue:  NewLIFOQueue(),
}

func auth(w http.ResponseWriter, r *http.Request) {
	// TODO: Auth mech
}

func processPlayerMessage(message []byte, p model.Player) {
	m := &model.ClientMessage{}
	json.Unmarshal(message, m)
	r := &model.ClientMessage{ModelType: model.ModelType{MsgType: model.CLIENTMESSAGE},
		Msg: m.Msg, Sender: p.ID}

	gh.broadcast <- r

}

func processPlayerAction(message []byte, p model.Player) {
	m := &model.EntityAction{}
	json.Unmarshal(message, m)
	switch m.Action {

	// Trying to move a single entity
	case model.ENTITYACTIONMOVE:
		r := &model.EntityAction{
			ModelType: model.ModelType{MsgType: model.ENTITYACTION},
			Entity: model.Entity{
				OwnerID: p.ID,
				ID:      m.ID,
			},
			Action: model.ENTITYACTIONMOVE,
			EntityMove: model.EntityMove{
				Direction: m.Direction,
				Distance:  m.Distance},
		}

		log.Debug("Move action:", m)
		gh.entityaction <- r
	}
}

func game(w http.ResponseWriter, r *http.Request) {
	c, err := upgrader.Upgrade(w, r, nil)

	if err != nil {
		log.Warn("UPGRADE ERROR:", err)
		return
	}

	// TODO: Returning player?
	player := model.NewPlayer(c)
	gh.register <- player
	mHolder := &model.ModelType{}

	for {

		// Get Client message
		_, message, err := c.ReadMessage()
		if err != nil {
			log.Warn(err)
			gh.unregister <- *player
			break
		}

		// What type of message are they sending?
		json.Unmarshal(message, mHolder)
		log.Info(mHolder.MsgType)

		// Determine message type so we can build proper validated messages to go to the hub
		switch mHolder.MsgType {

		// Receive client messages such as chatting to other players
		case model.CLIENTMESSAGE:
			processPlayerMessage(message, *player)

		// Entity Attacking moving etc
		case model.ENTITYACTION:
			processPlayerAction(message, *player)

		// A bad message
		default:
			log.Warn("Bad Message:", mHolder, message)
			r := &model.ClientMessage{ModelType: model.ModelType{MsgType: model.CLIENTERROR},
				Msg: "Unknown Message Type", Sender: "Server"}
			gh.DirectMessage(r, player.ID)
		}
	}
}

func home(w http.ResponseWriter, r *http.Request) {
	homeTemplate.Execute(w, "ws://"+r.Host+"/game")
}

func main() {
	// TODO Command line
	file, e := ioutil.ReadFile("./example.json")

	if e != nil {
		log.Fatal("READ FILE ERROR:", e)
	}

	world := &model.WorldMap{}
	json.Unmarshal(file, world)
	// fmt.Println(world)
	gh.WorldMapped = *world

	addr := "localhost:8080" // TODO: COMMAND LINE
	http.HandleFunc("/game", game)
	http.HandleFunc("/", home)
	http.HandleFunc("/auth", auth)

	//game loop
	go gh.ServeGame()

	// run web server
	log.Fatal(http.ListenAndServe(addr, nil))
}
