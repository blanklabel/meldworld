package main

import (
	"html/template"
	"log"
	"net/http"

	"encoding/json"
	"fmt"
	"io/ioutil"
	"os"

	"github.com/gorilla/websocket"
)

type MsgType struct {
	MType string `json:"type"`
}

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
	mHolder := &MsgType{}

	for {

		// Get Client message
		_, message, err := c.ReadMessage()
		if err != nil {
			gh.unregister <- *player
			break
		}

		fmt.Println(json.Unmarshal(message, mHolder))

		// Determine message type
		switch mHolder.MType {

		// Receive client messages
		case "client.message":
			m := &ClientMessage{}
			json.Unmarshal(message, m)
			r := &ClientMessage{MsgType: "client.message", Msg: m.Msg, Sender: player.ID}

			gh.broadcast <- r
			if err != nil {
				gh.unregister <- *player
				break
			}
		default:
			fmt.Println("Bad Message:", mHolder, message)
			r := &ClientMessage{MsgType: "client.error", Msg: "Unknown Message Type"}
			gh.DirectMessage(r, player.ID)
			// fmt.Println(mHolder.MType)
		}
	}
}

func home(w http.ResponseWriter, r *http.Request) {
	homeTemplate.Execute(w, "ws://"+r.Host+"/game")
}

func main() {
	file, e := ioutil.ReadFile("./example.json")

	if e != nil {
		fmt.Printf("File error: %v\n", e)
		os.Exit(1)
	}

	world := &WorldMap{}
	json.Unmarshal(file, world)
	gh.WorldMapped = *world

	addr := "localhost:8080"
	// log.SetFlags(0)
	http.HandleFunc("/game", game)
	http.HandleFunc("/", home)
	http.HandleFunc("/auth", auth)

	//fmt.Println(world)

	//game loop
	go gh.ServeGame()

	// run web server
	log.Fatal(http.ListenAndServe(addr, nil))
}
