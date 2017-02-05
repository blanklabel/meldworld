package main

import (
	"html/template"
	"log"
	"net/http"

	"github.com/gorilla/websocket"
)

var upgrader = websocket.Upgrader{} // use default options
var homeTemplate = template.Must(template.ParseFiles("home.html"))

var gh = GameHub{
	clients:    make(map[string]*Player),
	broadcast:  make(chan *ClientMessage),
	register:   make(chan *Player),
	unregister: make(chan Player),
}

func auth(w http.ResponseWriter, r *http.Request) {
	// Auth mech
}

func game(w http.ResponseWriter, r *http.Request) {
	c, err := upgrader.Upgrade(w, r, nil)

	if err != nil {
		log.Print("upgrade:", err)
		return
	}

	player := NewPlayer(c)
	gh.register <- player

	for {
		_, message, err := c.ReadMessage()
		if err != nil {
			gh.unregister <- *player
			break
		}

		r := &ClientMessage{MsgType: "clientmessage", Msg: string(message), Sender: player.ID}
		gh.broadcast <- r
		if err != nil {
			gh.unregister <- *player
			break

		}
	}
}

func home(w http.ResponseWriter, r *http.Request) {
	homeTemplate.Execute(w, "ws://"+r.Host+"/game")
}

func main() {
	addr := "localhost:8080"
	// log.SetFlags(0)
	http.HandleFunc("/game", game)
	http.HandleFunc("/", home)
	http.HandleFunc("/auth", auth)

	//game loop
	go gh.ServeGame()

	// run web server
	log.Fatal(http.ListenAndServe(addr, nil))
}
