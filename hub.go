package main

import (
	"fmt"
	"log"

	"time"

	"github.com/blanklabel/meldworld/entity"
	"github.com/blanklabel/meldworld/mapper"
)

type ClientMessage struct {
	MsgType string `json:"type"`
	Msg     string `json:"msg"`
	Sender  string `json:"sender"`
}

type GameHub struct {
	clients    map[string]*Player
	broadcast  chan *ClientMessage
	register   chan *Player
	unregister chan Player
}

type WorldMap struct {
	mapper.MapObj
	entity.EntityObj
}

func (g *GameHub) AddNewClient(p *Player) {
	g.clients[p.ID] = p
	announcement := fmt.Sprintf("Player %s joined", p.ID)
	r := &ClientMessage{MsgType: "client.join", Msg: announcement, Sender: "Server"}
	g.Broadcast(r)
}

func (g *GameHub) RemoveClient(p Player) {
	g.clients[p.ID].conn.Close()
	delete(g.clients, p.ID)
	announcement := fmt.Sprintf("Player %s left", p.ID)
	r := &ClientMessage{MsgType: "client.leave", Msg: announcement, Sender: "Server"}
	g.Broadcast(r)
}

func (g GameHub) DirectMessage(msg *ClientMessage, userID string) {
	g.clients[userID].conn.WriteJSON(msg)
}

func (g GameHub) Broadcast(msg *ClientMessage) {

	for id, client := range g.clients {
		err := client.conn.WriteJSON(msg)
		if err != nil {
			log.Print("Didn't send")
		}
		fmt.Printf("sent msg type of '%s' contents '%s' to '%s'\n", msg.MsgType, msg.Msg, id)
	}
}

func (g GameHub) ServeGame() {

	//running at 60 FPS everyone get the ticks
	frameNS := time.Duration(int(1e9) / 60)
	clk := time.NewTicker(frameNS)

	// Run forever
	for {
		select {
		// main loop
		case <-clk.C:
			// ummm?
			break
		case c := <-gh.register:
			gh.AddNewClient(c)
			break
		case c := <-gh.unregister:
			gh.RemoveClient(c)
			break
		case c := <-gh.broadcast:
			gh.Broadcast(c)
			break
		}
	}
}
